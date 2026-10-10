use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use std::{iter, result};

use async_trait::async_trait;
use futures_util::future::join_all;
use futures_util::{StreamExt, stream};
use matrix_sdk::deserialized_responses::SyncOrStrippedState;
use matrix_sdk::ruma::api::client::state::get_state_event_for_key;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::events::space::parent::SpaceParentEventContent;
use matrix_sdk::ruma::events::{GlobalAccountDataEventType, StateEventType, SyncStateEvent};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{OwnedMxcUri, OwnedRoomId};
use matrix_sdk::{Client, HttpError, Room, RoomState};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};
use tokio::task::JoinSet;

use super::session::ClientHandle;
use crate::domain::room::RoomId;
use crate::domain::sticker::{PackId, StickerImage, StickerPack};
use crate::error::{AppError, Result};
use crate::ports::matrix::{StickerCatalog, StickerPort};

const PACK_ROOMS_TYPES: [&str; 2] = ["m.image_pack.rooms", "im.ponies.emote_rooms"];
const ACCOUNT_PACK_TYPES: [&str; 2] = ["m.image_pack", "im.ponies.user_emotes"];
const ROOM_PACK_TYPES: [&str; 2] = ["m.room.image_pack", "im.ponies.room_emotes"];
const STICKER_EVENT_TYPE: &str = "m.sticker";
const STICKER_USAGE: &str = "sticker";
const MXC_SCHEME: &str = "mxc://";
const MAX_INFLIGHT_FETCHES: usize = 8;
const MAX_CANONICAL_SPACES: usize = 8;
const PACK_FRESHNESS: Duration = Duration::from_mins(10);

type StickerSources = StdMutex<HashMap<PackId, PackSources>>;
type PackSources = HashMap<String, StickerSource>;
type PackCache = StdMutex<HashMap<PackRef, CachedPack>>;

#[derive(Clone, PartialEq, Eq, Hash)]
struct PackRef {
    room: OwnedRoomId,
    state_key: String,
}

impl PackRef {
    fn pack_id(&self) -> PackId {
        PackId::new(format!("room:{}:{}", self.room, self.state_key))
    }
}

#[derive(Clone)]
struct CachedPack {
    fetched_at: Instant,
    pack: Option<StickerPack>,
}

enum PackFetch {
    Found(StickerPack, PackSources),
    Absent,
    Unreachable,
}

enum PackState {
    Published(PackDto),
    Missing,
    Unreachable,
}

struct StickerSource {
    body: String,
    url: String,
    info: Option<Map<String, Value>>,
}

#[derive(Deserialize, Default)]
struct PackRoomsDto {
    #[serde(default, deserialize_with = "lenient")]
    rooms: Option<BTreeMap<String, Value>>,
}

#[derive(Deserialize)]
struct PackDto {
    #[serde(default, deserialize_with = "lenient")]
    images: Option<BTreeMap<String, Value>>,
    #[serde(default, deserialize_with = "lenient")]
    pack: Option<PackInfoDto>,
}

#[derive(Deserialize)]
struct PackImageDto {
    url: String,
    #[serde(default, deserialize_with = "lenient")]
    body: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    info: Option<Map<String, Value>>,
    #[serde(default, deserialize_with = "lenient")]
    usage: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct PackInfoDto {
    #[serde(default, deserialize_with = "lenient")]
    display_name: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    avatar_url: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    usage: Option<Vec<String>>,
}

struct PackDefaults {
    title: String,
    avatar: Option<String>,
}

fn lenient<'de, D, T>(deserializer: D) -> result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(T::deserialize(Value::deserialize(deserializer)?).ok())
}

fn is_mxc(url: &str) -> bool {
    url.starts_with(MXC_SCHEME)
}

fn allows_stickers(usage: Option<&Vec<String>>) -> bool {
    usage.is_none_or(|kinds| kinds.is_empty() || kinds.iter().any(|kind| kind == STICKER_USAGE))
}

impl PackDto {
    fn into_pack(self, id: PackId, defaults: PackDefaults) -> Option<(StickerPack, PackSources)> {
        if !allows_stickers(self.pack.as_ref().and_then(|p| p.usage.as_ref())) {
            return None;
        }
        let (title, avatar) = self
            .pack
            .map(|info| (info.display_name, info.avatar_url))
            .unwrap_or_default();
        let title = title
            .filter(|name| !name.is_empty())
            .unwrap_or(defaults.title);
        let avatar = avatar.filter(|url| is_mxc(url)).or(defaults.avatar);

        let mut images = Vec::new();
        let mut sources = PackSources::new();
        for (shortcode, image) in self.images.unwrap_or_default() {
            let Ok(image) = PackImageDto::deserialize(image) else {
                continue;
            };
            if !allows_stickers(image.usage.as_ref()) || !is_mxc(&image.url) {
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
        let pack = StickerPack {
            id,
            title,
            avatar,
            images,
        };
        Some((pack, sources))
    }
}

pub(super) struct MatrixStickers {
    matrix: Arc<ClientHandle>,
    sources: Arc<StickerSources>,
    packs: PackCache,
}

impl MatrixStickers {
    pub(super) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self {
            matrix,
            sources: Arc::new(StdMutex::new(HashMap::new())),
            packs: StdMutex::new(HashMap::new()),
        }
    }

    fn remember_pack(&self, id: &PackId, sources: PackSources) {
        if let Ok(mut cache) = self.sources.lock() {
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
            let defaults = PackDefaults {
                title: "Your stickers".to_owned(),
                avatar: None,
            };
            if let Ok(dto) = raw.deserialize_as_unchecked::<PackDto>()
                && let Some(pack) = dto.into_pack(PackId::new("account"), defaults)
            {
                return Some(pack);
            }
        }
        None
    }

    async fn pack_references(&self, client: &Client) -> Vec<PackRef> {
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
            let refs: Vec<PackRef> = dto
                .rooms
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(room, keys)| {
                    let room: OwnedRoomId = room.try_into().ok()?;
                    let keys = BTreeMap::<String, Value>::deserialize(keys).ok()?;
                    Some((room, keys))
                })
                .flat_map(|(room, keys)| {
                    keys.into_keys()
                        .map(move |state_key| PackRef {
                            room: room.clone(),
                            state_key,
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            if !refs.is_empty() {
                return refs;
            }
        }
        Vec::new()
    }

    async fn room_packs(&self, client: &Client, references: Vec<PackRef>) -> Vec<StickerPack> {
        let packs: Vec<Option<StickerPack>> = stream::iter(references)
            .map(|reference| self.room_pack(client, reference))
            .buffered(MAX_INFLIGHT_FETCHES)
            .collect()
            .await;
        packs.into_iter().flatten().collect()
    }

    async fn room_pack(&self, client: &Client, reference: PackRef) -> Option<StickerPack> {
        if let Some(cached) = self.fresh_pack(&reference) {
            return cached.pack;
        }
        let pack = match fetch_room_pack(client, &reference).await {
            PackFetch::Found(pack, sources) => {
                self.remember_pack(&pack.id, sources);
                Some(pack)
            }
            PackFetch::Absent => None,
            PackFetch::Unreachable => return None,
        };
        self.cache_pack(reference, pack.clone());
        pack
    }

    fn fresh_pack(&self, reference: &PackRef) -> Option<CachedPack> {
        self.packs
            .lock()
            .ok()?
            .get(reference)
            .filter(|cached| cached.fetched_at.elapsed() < PACK_FRESHNESS)
            .cloned()
    }

    fn cache_pack(&self, reference: PackRef, pack: Option<StickerPack>) {
        if let Ok(mut packs) = self.packs.lock() {
            packs.insert(
                reference,
                CachedPack {
                    fetched_at: Instant::now(),
                    pack,
                },
            );
        }
    }
}

fn room_defaults(client: &Client, room_id: &OwnedRoomId) -> PackDefaults {
    let room = client.get_room(room_id);
    PackDefaults {
        title: room
            .as_ref()
            .and_then(|room| room.cached_display_name())
            .map_or_else(|| room_id.to_string(), |name| name.to_string()),
        avatar: room
            .as_ref()
            .and_then(|room| room.avatar_url())
            .map(|url| url.to_string()),
    }
}

async fn fetch_room_pack(client: &Client, reference: &PackRef) -> PackFetch {
    let answers =
        join_all(ROOM_PACK_TYPES.map(|event_type| fetch_pack_state(client, reference, event_type)))
            .await;

    let mut fetch = PackFetch::Absent;
    for answer in answers {
        match answer {
            PackState::Published(dto) => {
                let defaults = room_defaults(client, &reference.room);
                if let Some((pack, sources)) = dto.into_pack(reference.pack_id(), defaults) {
                    return PackFetch::Found(pack, sources);
                }
            }
            PackState::Missing => {}
            PackState::Unreachable => fetch = PackFetch::Unreachable,
        }
    }
    fetch
}

async fn fetch_pack_state(client: &Client, reference: &PackRef, event_type: &str) -> PackState {
    let request = get_state_event_for_key::v3::Request::new(
        reference.room.clone(),
        StateEventType::from(event_type),
        reference.state_key.clone(),
    );
    match client.send(request).await {
        Ok(response) => response
            .into_content()
            .deserialize_as_unchecked::<PackDto>()
            .map_or(PackState::Missing, PackState::Published),
        Err(e) if rules_out_a_pack(&e) => PackState::Missing,
        Err(e) => {
            tracing::debug!(
                room = %reference.room,
                event_type,
                "an image pack could not be fetched: {e}"
            );
            PackState::Unreachable
        }
    }
}

fn rules_out_a_pack(error: &HttpError) -> bool {
    matches!(
        error.client_api_error_kind(),
        Some(ErrorKind::NotFound | ErrorKind::Forbidden)
    )
}

async fn canonical_spaces(client: &Client, room: &Room) -> Vec<OwnedRoomId> {
    let mut spaces = Vec::new();
    let mut seen = HashSet::from([room.room_id().to_owned()]);
    let mut child = room.clone();
    while spaces.len() < MAX_CANONICAL_SPACES {
        let Some(parent) = canonical_parent(&child).await else {
            break;
        };
        if !seen.insert(parent.clone()) {
            break;
        }
        let Some(space) = client
            .get_room(&parent)
            .filter(|space| space.state() == RoomState::Joined)
        else {
            break;
        };
        spaces.push(parent);
        child = space;
    }
    spaces
}

async fn canonical_parent(room: &Room) -> Option<OwnedRoomId> {
    let parents = room
        .get_state_events_static::<SpaceParentEventContent>()
        .await
        .ok()?;
    parents
        .into_iter()
        .filter_map(|raw| match raw.deserialize().ok()? {
            SyncOrStrippedState::Sync(SyncStateEvent::Original(event))
                if event.content.canonical && !event.content.via.is_empty() =>
            {
                Some(event.state_key)
            }
            _ => None,
        })
        .min()
}

#[async_trait]
impl StickerPort for MatrixStickers {
    async fn catalog(&self, room_id: &RoomId) -> Result<StickerCatalog> {
        let client = self.matrix.client().await?;
        let room = self.matrix.room(room_id).await?;

        let mut packs: Vec<StickerPack> = Vec::new();

        if let Some((pack, sources)) = self.account_pack(&client).await {
            self.remember_pack(&pack.id, sources);
            packs.push(pack);
        }

        let mut references = self.pack_references(&client).await;
        let spaces = canonical_spaces(&client, &room).await;
        for room in iter::once(room.room_id().to_owned()).chain(spaces) {
            let own_pack = PackRef {
                room,
                state_key: String::new(),
            };
            if !references.contains(&own_pack) {
                references.push(own_pack);
            }
        }
        packs.extend(self.room_packs(&client, references).await);

        Ok(StickerCatalog {
            packs,
            room_encrypted: room.encryption_state().is_encrypted(),
        })
    }

    async fn prefetch(&self, mxcs: &[String]) -> usize {
        let Ok(client) = self.matrix.client().await else {
            return 0;
        };

        let mut fetched = 0;
        let mut tasks: JoinSet<bool> = JoinSet::new();
        for mxc in mxcs {
            let uri: OwnedMxcUri = mxc.as_str().into();
            let client = client.clone();
            let media = Arc::clone(self.matrix.media());
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
        let content = Raw::new(&content)
            .map_err(|e| AppError::Other(e.to_string()))?
            .cast_unchecked();
        self.matrix
            .room(room_id)
            .await?
            .send_queue()
            .send_raw(content, STICKER_EVENT_TYPE.to_owned())
            .await
            .map(|_handle| ())
            .map_err(|e| AppError::Other(e.to_string()))
    }
}

impl MatrixStickers {
    fn sticker_event_content(
        &self,
        pack: &PackId,
        shortcode: &str,
        in_reply_to: Option<&str>,
    ) -> Result<Value> {
        let cache = self
            .sources
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
            source.info.clone().map_or_else(|| json!({}), Value::Object),
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
