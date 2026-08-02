use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use matrix_sdk::Client;
use matrix_sdk::ruma::api::client::state::get_state_event_for_key;
use matrix_sdk::ruma::events::{GlobalAccountDataEventType, StateEventType};
use matrix_sdk::ruma::{OwnedMxcUri, OwnedRoomId};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::task::JoinSet;

use super::AuthedMatrix;
use crate::domain::models::{PackId, RoomId, StickerImage, StickerPack};
use crate::error::{AppError, Result};
use crate::ports::matrix::{StickerCatalog, StickerPort};

const PACK_ROOMS_TYPES: [&str; 2] = ["m.image_pack.rooms", "im.ponies.emote_rooms"];
const ACCOUNT_PACK_TYPES: [&str; 2] = ["m.image_pack", "im.ponies.user_emotes"];
const ROOM_PACK_TYPES: [&str; 2] = ["m.room.image_pack", "im.ponies.room_emotes"];
const STICKER_USAGE: &str = "sticker";
const MAX_INFLIGHT_FETCHES: usize = 8;

pub(super) type StickerSources = StdMutex<HashMap<PackId, PackSources>>;
type PackSources = HashMap<String, StickerSource>;

pub(super) struct StickerSource {
    body: String,
    url: String,
    info: Option<Value>,
}

#[derive(Deserialize, Default)]
struct PackRoomsDto {
    #[serde(default)]
    rooms: BTreeMap<String, BTreeMap<String, Value>>,
}

#[derive(Deserialize)]
struct PackDto {
    #[serde(default)]
    images: BTreeMap<String, PackImageDto>,
    #[serde(default)]
    pack: Option<PackInfoDto>,
}

#[derive(Deserialize)]
struct PackImageDto {
    url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    info: Option<Value>,
    #[serde(default)]
    usage: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct PackInfoDto {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    usage: Option<Vec<String>>,
}

fn allows_stickers(usage: Option<&Vec<String>>) -> bool {
    usage.is_none_or(|kinds| kinds.is_empty() || kinds.iter().any(|kind| kind == STICKER_USAGE))
}

impl PackDto {
    fn into_pack(self, id: PackId, fallback_title: &str) -> Option<(StickerPack, PackSources)> {
        if !allows_stickers(self.pack.as_ref().and_then(|p| p.usage.as_ref())) {
            return None;
        }
        let title = self
            .pack
            .and_then(|p| p.display_name)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| fallback_title.to_owned());

        let mut images = Vec::new();
        let mut sources = PackSources::new();
        for (shortcode, image) in self.images {
            if !allows_stickers(image.usage.as_ref()) || !image.url.starts_with("mxc://") {
                continue;
            }
            let body = image
                .body
                .filter(|body| !body.is_empty())
                .unwrap_or_else(|| shortcode.clone());
            sources.insert(
                shortcode.clone(),
                StickerSource {
                    body: body.clone(),
                    url: image.url.clone(),
                    info: image.info,
                },
            );
            images.push(StickerImage {
                shortcode,
                body,
                mxc: image.url,
            });
        }

        if images.is_empty() {
            return None;
        }
        Some((StickerPack { id, title, images }, sources))
    }
}

impl AuthedMatrix {
    fn remember_pack(&self, id: &PackId, sources: PackSources) {
        if let Ok(mut cache) = self.sticker_sources.lock() {
            cache.insert(id.clone(), sources);
        }
    }

    async fn account_pack(&self, client: &Client) -> Option<(StickerPack, PackSources)> {
        for event_type in ACCOUNT_PACK_TYPES {
            let Ok(Some(raw)) = client
                .account()
                .account_data_raw(GlobalAccountDataEventType::from(event_type))
                .await
            else {
                continue;
            };
            if let Ok(dto) = raw.deserialize_as_unchecked::<PackDto>()
                && let Some(pack) = dto.into_pack(PackId::new("account"), "Your stickers")
            {
                return Some(pack);
            }
        }
        None
    }

    async fn pack_rooms(&self, client: &Client) -> Vec<(OwnedRoomId, String)> {
        for event_type in PACK_ROOMS_TYPES {
            let Ok(Some(raw)) = client
                .account()
                .account_data_raw(GlobalAccountDataEventType::from(event_type))
                .await
            else {
                continue;
            };
            let Ok(dto) = raw.deserialize_as_unchecked::<PackRoomsDto>() else {
                continue;
            };
            let refs: Vec<(OwnedRoomId, String)> = dto
                .rooms
                .into_iter()
                .filter_map(|(room, keys)| {
                    let room: OwnedRoomId = room.try_into().ok()?;
                    Some((room, keys))
                })
                .flat_map(|(room, keys)| {
                    keys.into_keys()
                        .map(move |key| (room.clone(), key))
                        .collect::<Vec<_>>()
                })
                .collect();
            if !refs.is_empty() {
                return refs;
            }
        }
        Vec::new()
    }

    async fn room_pack(
        &self,
        client: &Client,
        room_id: &OwnedRoomId,
        state_key: &str,
    ) -> Option<(StickerPack, PackSources)> {
        let fallback = client
            .get_room(room_id)
            .and_then(|room| room.cached_display_name())
            .map_or_else(|| room_id.to_string(), |name| name.to_string());

        for event_type in ROOM_PACK_TYPES {
            let request = get_state_event_for_key::v3::Request::new(
                room_id.clone(),
                StateEventType::from(event_type),
                state_key.to_owned(),
            );
            let Ok(response) = client.send(request).await else {
                continue;
            };
            if let Ok(dto) = response
                .into_content()
                .deserialize_as_unchecked::<PackDto>()
            {
                let id = PackId::new(format!("room:{room_id}:{state_key}"));
                if let Some(pack) = dto.into_pack(id, &fallback) {
                    return Some(pack);
                }
            }
        }
        None
    }
}

#[async_trait]
impl StickerPort for AuthedMatrix {
    async fn catalog(&self, room_id: &RoomId) -> Result<StickerCatalog> {
        let client = self.client().await?;
        let room = self.room(room_id).await?;

        let mut packs: Vec<StickerPack> = Vec::new();

        if let Some((pack, sources)) = self.account_pack(&client).await {
            self.remember_pack(&pack.id, sources);
            packs.push(pack);
        }

        let mut references = self.pack_rooms(&client).await;
        references.push((room.room_id().to_owned(), String::new()));

        for (pack_room, state_key) in references {
            let Some((pack, sources)) = self.room_pack(&client, &pack_room, &state_key).await
            else {
                continue;
            };
            if packs.iter().any(|known| known.id == pack.id) {
                continue;
            }
            self.remember_pack(&pack.id, sources);
            packs.push(pack);
        }

        Ok(StickerCatalog {
            packs,
            room_encrypted: room.encryption_state().is_encrypted(),
        })
    }

    async fn prefetch(&self, mxcs: &[String]) -> usize {
        let Ok(client) = self.client().await else {
            return 0;
        };

        let mut fetched = 0;
        let mut tasks: JoinSet<bool> = JoinSet::new();
        for mxc in mxcs {
            let uri: OwnedMxcUri = mxc.as_str().into();
            let client = client.clone();
            let media = Arc::clone(&self.media);
            tasks.spawn(async move { media.fetch_sticker_by_mxc(&client, uri).await.is_some() });
            if tasks.len() >= MAX_INFLIGHT_FETCHES {
                fetched += usize::from(matches!(tasks.join_next().await, Some(Ok(true))));
            }
        }
        while let Some(result) = tasks.join_next().await {
            fetched += usize::from(matches!(result, Ok(true)));
        }
        fetched
    }

    async fn send_sticker(
        &self,
        room_id: &RoomId,
        pack: &PackId,
        shortcode: &str,
        in_reply_to: Option<&str>,
    ) -> Result<()> {
        let content = self.sticker_event_content(pack, shortcode, in_reply_to)?;
        self.room(room_id)
            .await?
            .send_raw("m.sticker", content)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        Ok(())
    }
}

impl AuthedMatrix {
    fn sticker_event_content(
        &self,
        pack: &PackId,
        shortcode: &str,
        in_reply_to: Option<&str>,
    ) -> Result<Value> {
        let cache = self
            .sticker_sources
            .lock()
            .map_err(|_| AppError::Other("The sticker cache is poisoned".into()))?;
        let source = cache
            .get(pack)
            .and_then(|sources| sources.get(shortcode))
            .ok_or_else(|| AppError::Other(format!("Unknown sticker {pack}/{shortcode}")))?;

        let mut content = Map::new();
        content.insert("body".to_owned(), json!(source.body));
        content.insert("url".to_owned(), json!(source.url));
        content.insert(
            "info".to_owned(),
            source.info.clone().unwrap_or_else(|| json!({})),
        );
        if let Some(event_id) = in_reply_to {
            content.insert(
                "m.relates_to".to_owned(),
                json!({ "m.in_reply_to": { "event_id": event_id } }),
            );
        }
        Ok(Value::Object(content))
    }
}
