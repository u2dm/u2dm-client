use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use std::{iter, result};

use async_trait::async_trait;
use futures_util::future::join_all;
use futures_util::{StreamExt, stream};
use matrix_sdk::config::RequestConfig;
use matrix_sdk::deserialized_responses::SyncOrStrippedState;
use matrix_sdk::ruma::api::client::config::{get_global_account_data, set_global_account_data};
use matrix_sdk::ruma::api::client::state::get_state_event_for_key;
use matrix_sdk::ruma::api::client::sync::sync_events::v5;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::events::space::parent::SpaceParentEventContent;
use matrix_sdk::ruma::events::{
    AnyGlobalAccountDataEventContent, AnySyncStateEvent, GlobalAccountDataEventType,
    StateEventType, SyncStateEvent,
};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{OwnedMxcUri, OwnedRoomId, OwnedUserId, assign};
use matrix_sdk::{Client, HttpError, Room, RoomState};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::session::ClientHandle;
use super::user_info::own_user;
use crate::domain::image_pack::{ImagePack, ImageUsage, PackId, PackImage, PackOrigin, PackTitle};
use crate::domain::room::RoomId;
use crate::error::{AppError, Result};
use crate::ports::matrix::{ImagePackPort, PackCatalog};

const PACK_ROOMS_TYPES: [&str; 2] = ["m.image_pack.rooms", "im.ponies.emote_rooms"];
const ACCOUNT_PACK_TYPES: [&str; 2] = ["m.image_pack", "im.ponies.user_emotes"];
const ROOM_PACK_TYPES: [&str; 2] = ["m.room.image_pack", "im.ponies.room_emotes"];
const STICKER_EVENT_TYPE: &str = "m.sticker";
const EMOTICON_USAGE: &str = "emoticon";
const STICKER_USAGE: &str = "sticker";
const MXC_SCHEME: &str = "mxc://";
const MAX_INFLIGHT_FETCHES: usize = 8;
const MAX_CANONICAL_SPACES: usize = 8;
const PACK_FRESHNESS: Duration = Duration::from_mins(10);
const PACK_SCAN_CONNECTION: &str = "u2dm-packs";
const ANY_STATE_KEY: &str = "*";
const ROOMS_FIELD: &str = "rooms";

type ImageSources = StdMutex<HashMap<PackId, PackSources>>;
type PackSources = HashMap<String, ImageSource>;
type PackCache = StdMutex<HashMap<PackRef, CachedPack>>;
type ScanCache = StdMutex<HashMap<OwnedRoomId, ScannedRoom>>;
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

#[derive(Clone)]
struct ScannedRoom {
    fetched_at: Instant,
    packs: KeyedPacks,
}

struct UnechoedWrite {
    written_at: Instant,
    earlier: Vec<Option<Value>>,
    content: Value,
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

#[derive(Deserialize)]
struct ScannedStateDto {
    #[serde(rename = "type")]
    event_type: String,
    state_key: String,
    #[serde(default)]
    content: Value,
}

struct PackDefaults {
    title: PackTitle,
    origin: PackOrigin,
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

fn usage_named(kinds: Option<&Vec<String>>) -> Option<ImageUsage> {
    let kinds = kinds?;
    ImageUsage::named(
        kinds.iter().any(|kind| kind == EMOTICON_USAGE),
        kinds.iter().any(|kind| kind == STICKER_USAGE),
    )
}

impl PackRoomsDto {
    fn references(self) -> Vec<PackRef> {
        self.rooms
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
            .collect()
    }
}

impl PackDto {
    fn into_pack(self, id: PackId, defaults: PackDefaults) -> Option<(ImagePack, PackSources)> {
        let (title, avatar, pack_usage) = self
            .pack
            .map(|info| {
                let usage = usage_named(info.usage.as_ref());
                (info.display_name, info.avatar_url, usage)
            })
            .unwrap_or_default();
        let title = title
            .filter(|name| !name.is_empty())
            .map_or(defaults.title, PackTitle::Named);
        let avatar = avatar.filter(|url| is_mxc(url)).or(defaults.avatar);

        let mut images = Vec::new();
        let mut sources = PackSources::new();
        for (shortcode, image) in self.images.unwrap_or_default() {
            let Ok(image) = PackImageDto::deserialize(image) else {
                continue;
            };
            if !is_mxc(&image.url) {
                continue;
            }
            let usage = ImageUsage::resolved(usage_named(image.usage.as_ref()), pack_usage);
            let body = image
                .body
                .filter(|body| !body.is_empty())
                .unwrap_or_else(|| shortcode.clone());
            sources.insert(
                shortcode.clone(),
                ImageSource {
                    body: body.clone(),
                    url: image.url.clone(),
                    info: image.info,
                },
            );
            images.push(PackImage {
                shortcode,
                body,
                mxc: image.url,
                usage,
            });
        }

        if images.is_empty() {
            return None;
        }
        let pack = ImagePack {
            id,
            title,
            origin: defaults.origin,
            avatar,
            images,
        };
        Some((pack, sources))
    }
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

    async fn pack_references(&self, client: &Client) -> Vec<PackRef> {
        let mut references = BTreeSet::new();
        for event_type in PACK_ROOMS_TYPES {
            let stored = stored_account_data(client, event_type).await;
            let dto = match self.unechoed_write(event_type, stored.as_ref()) {
                Some(written) => PackRoomsDto::deserialize(written).ok(),
                None => stored.and_then(|raw| raw.deserialize_as_unchecked::<PackRoomsDto>().ok()),
            };
            if let Some(dto) = dto {
                references.extend(dto.references());
            }
        }
        references.into_iter().collect()
    }

    fn unechoed_write(
        &self,
        event_type: &str,
        stored: Option<&Raw<AnyGlobalAccountDataEventContent>>,
    ) -> Option<Value> {
        let stored = stored.and_then(|raw| raw.deserialize_as_unchecked::<Value>().ok());
        let mut unechoed = self.unechoed.lock().ok()?;
        let write = unechoed.get(event_type)?;
        let catching_up = write.earlier.contains(&stored);
        if catching_up && write.written_at.elapsed() < PACK_FRESHNESS {
            return Some(write.content.clone());
        }
        unechoed.remove(event_type);
        None
    }

    fn remember_write(&self, event_type: &'static str, stored: Option<Value>, content: Value) {
        let Ok(mut unechoed) = self.unechoed.lock() else {
            return;
        };
        let mut earlier: Vec<Option<Value>> = unechoed
            .remove(event_type)
            .filter(|previous| previous.written_at.elapsed() < PACK_FRESHNESS)
            .map(|previous| {
                let mut earlier = previous.earlier;
                earlier.push(Some(previous.content));
                earlier
            })
            .unwrap_or_default();
        if !earlier.contains(&stored) {
            earlier.push(stored);
        }
        unechoed.insert(
            event_type,
            UnechoedWrite {
                written_at: Instant::now(),
                earlier,
                content,
            },
        );
    }

    fn remember_references<'a>(&self, packs: impl IntoIterator<Item = &'a (PackRef, ImagePack)>) {
        if let Ok(mut references) = self.references.lock() {
            references.extend(
                packs
                    .into_iter()
                    .map(|(reference, pack)| (pack.id.clone(), reference.clone())),
            );
        }
    }

    fn reference_of(&self, pack: &PackId) -> Result<PackRef> {
        self.references
            .lock()
            .ok()
            .and_then(|references| references.get(pack).cloned())
            .ok_or_else(|| AppError::Other(format!("Unknown image pack {pack}")))
    }

    async fn write_reference(&self, reference: &PackRef, global: bool) -> Result<()> {
        let _write = self
            .reference_writes
            .acquire()
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        let client = self.matrix.client().await?;
        let own = own_user(&client)?;
        let mut updates = Vec::new();
        for event_type in PACK_ROOMS_TYPES {
            let current = fetch_pack_rooms(&client, &own, event_type).await?;
            if let Some(content) = referencing(current, reference, global, event_type)? {
                updates.push((event_type, content));
            }
        }
        for (event_type, content) in updates {
            let stored = stored_account_data(&client, event_type)
                .await
                .and_then(|raw| raw.deserialize_as_unchecked::<Value>().ok());
            put_pack_rooms(&client, &own, event_type, &content).await?;
            self.remember_write(event_type, stored, content);
        }
        Ok(())
    }

    async fn room_packs(&self, client: &Client, references: Vec<PackRef>) -> ReferencedPacks {
        let packs: Vec<Option<(PackRef, ImagePack)>> = stream::iter(references)
            .map(|reference| async move {
                let pack = self.room_pack(client, reference.clone()).await?;
                Some((reference, pack))
            })
            .buffered(MAX_INFLIGHT_FETCHES)
            .collect()
            .await;
        packs.into_iter().flatten().collect()
    }

    async fn room_pack(&self, client: &Client, reference: PackRef) -> Option<ImagePack> {
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

    fn cache_pack(&self, reference: PackRef, pack: Option<ImagePack>) {
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

    async fn local_packs(&self, client: &Client, rooms: &[OwnedRoomId]) -> ReferencedPacks {
        let mut scanned: HashMap<OwnedRoomId, KeyedPacks> = rooms
            .iter()
            .filter_map(|room| Some((room.clone(), self.fresh_scan(room)?)))
            .collect();
        let stale: Vec<OwnedRoomId> = rooms
            .iter()
            .filter(|room| !scanned.contains_key(*room))
            .cloned()
            .collect();
        if !stale.is_empty() {
            scanned.extend(self.scan(client, &stale).await);
        }

        let mut packs = Vec::new();
        for room in rooms {
            let mut found = scanned.remove(room).unwrap_or_default();
            if !found.iter().any(|(state_key, _)| state_key.is_empty()) {
                let unnamed = PackRef {
                    room: room.clone(),
                    state_key: String::new(),
                };
                if let Some(pack) = self.room_pack(client, unnamed).await {
                    found.insert(0, (String::new(), pack));
                }
            }
            packs.extend(found.into_iter().map(|(state_key, pack)| {
                let reference = PackRef {
                    room: room.clone(),
                    state_key,
                };
                (reference, pack)
            }));
        }
        packs
    }

    async fn scan(&self, client: &Client, rooms: &[OwnedRoomId]) -> Vec<(OwnedRoomId, KeyedPacks)> {
        let states = match scan_pack_state(client, rooms).await {
            Ok(states) => states,
            Err(e) => {
                tracing::debug!(rooms = rooms.len(), "image packs could not be scanned: {e}");
                return Vec::new();
            }
        };
        states
            .into_iter()
            .filter(|(room, _)| rooms.contains(room))
            .map(|(room, events)| {
                let packs = self.packs_in_state(client, &room, events);
                self.cache_scan(&room, packs.clone());
                (room, packs)
            })
            .collect()
    }

    fn packs_in_state(
        &self,
        client: &Client,
        room: &OwnedRoomId,
        events: Vec<Raw<AnySyncStateEvent>>,
    ) -> KeyedPacks {
        let mut published: BTreeMap<String, Vec<(usize, PackDto)>> = BTreeMap::new();
        for raw in events {
            let Ok(event) = raw.deserialize_as_unchecked::<ScannedStateDto>() else {
                continue;
            };
            let Some(generation) = ROOM_PACK_TYPES
                .iter()
                .position(|event_type| *event_type == event.event_type)
            else {
                continue;
            };
            if let Ok(dto) = PackDto::deserialize(event.content) {
                published
                    .entry(event.state_key)
                    .or_default()
                    .push((generation, dto));
            }
        }

        let mut packs = Vec::new();
        for (state_key, mut generations) in published {
            generations.sort_by_key(|(generation, _)| *generation);
            let reference = PackRef {
                room: room.clone(),
                state_key,
            };
            let found = generations.into_iter().find_map(|(_, dto)| {
                dto.into_pack(reference.pack_id(), room_defaults(client, room))
            });
            if let Some((pack, sources)) = found {
                self.remember_pack(&pack.id, sources);
                packs.push((reference.state_key, pack));
            }
        }
        packs
    }

    fn fresh_scan(&self, room: &OwnedRoomId) -> Option<KeyedPacks> {
        self.scans
            .lock()
            .ok()?
            .get(room)
            .filter(|scanned| scanned.fetched_at.elapsed() < PACK_FRESHNESS)
            .map(|scanned| scanned.packs.clone())
    }

    fn cache_scan(&self, room: &OwnedRoomId, packs: KeyedPacks) {
        if let Ok(mut scans) = self.scans.lock() {
            scans.insert(
                room.clone(),
                ScannedRoom {
                    fetched_at: Instant::now(),
                    packs,
                },
            );
        }
    }
}

fn room_defaults(client: &Client, room_id: &OwnedRoomId) -> PackDefaults {
    let room = client.get_room(room_id);
    PackDefaults {
        title: PackTitle::Named(
            room.as_ref()
                .and_then(|room| room.cached_display_name())
                .map_or_else(|| room_id.to_string(), |name| name.to_string()),
        ),
        origin: PackOrigin::Room,
        avatar: room
            .as_ref()
            .and_then(|room| room.avatar_url())
            .map(|url| url.to_string()),
    }
}

async fn scan_pack_state(
    client: &Client,
    rooms: &[OwnedRoomId],
) -> result::Result<BTreeMap<OwnedRoomId, Vec<Raw<AnySyncStateEvent>>>, HttpError> {
    let mut request = v5::Request::new();
    request.conn_id = Some(PACK_SCAN_CONNECTION.to_owned());
    request.timeout = Some(Duration::ZERO);
    for room in rooms {
        let subscription = assign!(v5::request::RoomSubscription::default(), {
            required_state: ROOM_PACK_TYPES
                .iter()
                .map(|event_type| (StateEventType::from(*event_type), ANY_STATE_KEY.to_owned()))
                .collect(),
        });
        request
            .room_subscriptions
            .insert(room.clone(), subscription);
    }
    let response = client
        .send(request)
        .with_request_config(RequestConfig::short_retry())
        .await?;
    Ok(response
        .rooms
        .into_iter()
        .map(|(room, details)| (room, details.required_state))
        .collect())
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

async fn fetch_pack_rooms(
    client: &Client,
    own: &OwnedUserId,
    event_type: &str,
) -> Result<Option<Value>> {
    let request = get_global_account_data::v3::Request::new(
        own.clone(),
        GlobalAccountDataEventType::from(event_type),
    );
    match client
        .send(request)
        .with_request_config(RequestConfig::short_retry())
        .await
    {
        Ok(response) => serde_json::from_str(response.account_data.json().get())
            .map(Some)
            .map_err(|e| AppError::Other(e.to_string())),
        Err(e) if e.client_api_error_kind() == Some(&ErrorKind::NotFound) => Ok(None),
        Err(e) => Err(AppError::Other(e.to_string())),
    }
}

async fn put_pack_rooms(
    client: &Client,
    own: &OwnedUserId,
    event_type: &str,
    content: &Value,
) -> Result<()> {
    let data = Raw::new(content)
        .map_err(|e| AppError::Other(e.to_string()))?
        .cast_unchecked();
    let request = set_global_account_data::v3::Request::new_raw(
        own.clone(),
        GlobalAccountDataEventType::from(event_type),
        data,
    );
    client
        .send(request)
        .with_request_config(RequestConfig::short_retry())
        .await
        .map(drop)
        .map_err(|e| AppError::Other(e.to_string()))
}

fn referencing(
    current: Option<Value>,
    reference: &PackRef,
    global: bool,
    event_type: &str,
) -> Result<Option<Value>> {
    let mut content = match current {
        Some(Value::Object(content)) => content,
        None if global => Map::new(),
        None => return Ok(None),
        Some(_) => return Err(unexpected_shape(event_type)),
    };
    let rooms = content
        .entry(ROOMS_FIELD)
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(rooms) = rooms else {
        return Err(unexpected_shape(event_type));
    };
    let changed = if global {
        add_reference(rooms, reference)
    } else {
        remove_reference(rooms, reference)
    }
    .ok_or_else(|| unexpected_shape(event_type))?;
    Ok(changed.then_some(Value::Object(content)))
}

fn add_reference(rooms: &mut Map<String, Value>, reference: &PackRef) -> Option<bool> {
    let keys = rooms
        .entry(reference.room.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(keys) = keys else {
        return None;
    };
    if keys.contains_key(&reference.state_key) {
        return Some(false);
    }
    keys.insert(reference.state_key.clone(), Value::Object(Map::new()));
    Some(true)
}

fn remove_reference(rooms: &mut Map<String, Value>, reference: &PackRef) -> Option<bool> {
    let room = reference.room.to_string();
    let Some(keys) = rooms.get_mut(&room) else {
        return Some(false);
    };
    let Value::Object(keys) = keys else {
        return None;
    };
    let removed = keys.remove(&reference.state_key).is_some();
    if removed && keys.is_empty() {
        rooms.remove(&room);
    }
    Some(removed)
}

fn unexpected_shape(event_type: &str) -> AppError {
    AppError::Other(format!("{event_type} is not shaped as the spec says"))
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
