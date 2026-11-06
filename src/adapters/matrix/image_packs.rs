mod references;
mod scan;
mod wire;

use std::collections::{HashMap, HashSet};
use std::iter;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use matrix_sdk::Client;
use matrix_sdk::ruma::events::{
    AnyGlobalAccountDataEventContent, GlobalAccountDataEventType, MessageLikeEventType,
};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{OwnedMxcUri, OwnedRoomId};
use serde_json::{Map, Value, json};
use tokio::sync::Semaphore;

use self::references::{ACCOUNT_PACK_TYPES, UnechoedWrite};
use self::scan::{ScanCache, canonical_spaces};
use self::wire::{PackDefaults, PackDto};
use super::media::count_fetched;
use super::session::ClientHandle;
use super::timeline::{refuse_oversized, refuse_unpermitted};
use crate::domain::image_pack::{ImagePack, PackId, PackOrigin, PackTitle};
use crate::domain::room::RoomId;
use crate::error::{AppError, Result};
use crate::ports::matrix::{ImagePackPort, PackCatalog};

const STICKER_EVENT_TYPE: &str = "m.sticker";

const MAX_INFLIGHT_FETCHES: usize = 8;

const MAX_CANONICAL_SPACES: usize = 8;

const PACK_FRESHNESS: Duration = Duration::from_mins(10);

type ImageSources = StdMutex<HashMap<PackId, PackSources>>;

type PackSources = HashMap<String, ImageSource>;

type PackCache = StdMutex<HashMap<PackRef, CachedPack>>;

type KeyedPacks = Vec<(String, ImagePack)>;

type ReferencedPacks = Vec<(PackRef, ImagePack)>;

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
    pack: Option<ImagePack>,
}

enum PackFetch {
    Found(ImagePack, PackSources),
    Absent,
    Unreachable,
}

enum PackState {
    Published(PackDto),
    Missing,
    Unreachable,
}

struct ImageSource {
    body: String,
    url: String,
    info: Option<Map<String, Value>>,
}

pub(super) struct MatrixImagePacks {
    matrix: Arc<ClientHandle>,
    sources: Arc<ImageSources>,
    packs: PackCache,
    scans: ScanCache,
    references: StdMutex<HashMap<PackId, PackRef>>,
    unechoed: StdMutex<HashMap<&'static str, UnechoedWrite>>,
    reference_writes: Semaphore,
}

impl MatrixImagePacks {
    pub(super) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self {
            matrix,
            sources: Arc::new(StdMutex::new(HashMap::new())),
            packs: StdMutex::new(HashMap::new()),
            scans: StdMutex::new(HashMap::new()),
            references: StdMutex::new(HashMap::new()),
            unechoed: StdMutex::new(HashMap::new()),
            reference_writes: Semaphore::new(1),
        }
    }

    fn remember_pack(&self, id: &PackId, sources: PackSources) {
        if let Ok(mut cache) = self.sources.lock() {
            cache.insert(id.clone(), sources);
        }
    }

    async fn account_pack(&self, client: &Client) -> Option<(ImagePack, PackSources)> {
        for event_type in ACCOUNT_PACK_TYPES {
            let Ok(Some(raw)) = client
                .account()
                .account_data_raw(GlobalAccountDataEventType::from(event_type))
                .await
            else {
                continue;
            };
            let defaults = PackDefaults {
                title: PackTitle::Personal,
                origin: PackOrigin::Personal,
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
}

async fn stored_account_data(
    client: &Client,
    event_type: &str,
) -> Option<Raw<AnyGlobalAccountDataEventContent>> {
    client
        .account()
        .account_data_raw(GlobalAccountDataEventType::from(event_type))
        .await
        .ok()
        .flatten()
}

#[async_trait]
impl ImagePackPort for MatrixImagePacks {
    async fn catalog(&self, room_id: &RoomId) -> Result<PackCatalog> {
        let client = self.matrix.client().await?;
        let room = self.matrix.room(room_id).await?;

        let mut packs: Vec<ImagePack> = Vec::new();

        if let Some((pack, sources)) = self.account_pack(&client).await {
            self.remember_pack(&pack.id, sources);
            packs.push(pack);
        }

        let references = self.pack_references(&client).await;
        let listed: HashSet<PackRef> = references.iter().cloned().collect();
        let global = self.room_packs(&client, references).await;

        let spaces = canonical_spaces(&client, &room).await;
        let rooms: Vec<OwnedRoomId> = iter::once(room.room_id().to_owned())
            .chain(spaces)
            .collect();
        let local: ReferencedPacks = self
            .local_packs(&client, &rooms)
            .await
            .into_iter()
            .filter(|(reference, _)| !listed.contains(reference))
            .collect();

        self.remember_references(global.iter().chain(&local));
        packs.extend(global.into_iter().map(|(_, pack)| ImagePack {
            origin: PackOrigin::Global,
            ..pack
        }));
        packs.extend(local.into_iter().map(|(_, pack)| pack));

        Ok(PackCatalog {
            packs,
            room_encrypted: room.encryption_state().is_encrypted(),
        })
    }

    async fn prefetch(&self, mxcs: &[String]) -> usize {
        let Ok(client) = self.matrix.client().await else {
            return 0;
        };
        count_fetched(mxcs, MAX_INFLIGHT_FETCHES, |mxc| {
            let uri: OwnedMxcUri = mxc.into();
            let client = client.clone();
            let media = Arc::clone(self.matrix.media());
            async move { media.fetch_sticker_by_mxc(&client, uri).await.is_some() }
        })
        .await
    }

    async fn send_sticker(
        &self,
        room_id: &RoomId,
        pack: &PackId,
        shortcode: &str,
        in_reply_to: Option<&str>,
    ) -> Result<()> {
        let content = self.sticker_event_content(pack, shortcode, in_reply_to)?;
        let room = self.matrix.room(room_id).await?;
        refuse_unpermitted(&room, MessageLikeEventType::Sticker).await?;
        refuse_oversized(&room, &content, STICKER_EVENT_TYPE).await?;
        let content = Raw::new(&content)
            .map_err(AppError::other)?
            .cast_unchecked();
        room.send_queue()
            .send_raw(content, STICKER_EVENT_TYPE.to_owned())
            .await
            .map(|_handle| ())
            .map_err(AppError::other)
    }

    async fn set_global(&self, pack: &PackId, global: bool) -> Result<()> {
        let reference = self.reference_of(pack)?;
        self.write_reference(&reference, global).await
    }
}

impl MatrixImagePacks {
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
