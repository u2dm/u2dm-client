use std::collections::{BTreeMap, HashMap, HashSet};
use std::result;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use futures_util::{StreamExt, stream};
use matrix_sdk::config::RequestConfig;
use matrix_sdk::deserialized_responses::SyncOrStrippedState;
use matrix_sdk::ruma::api::client::state::get_state_event_for_key;
use matrix_sdk::ruma::api::client::sync::sync_events::v5;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::events::space::parent::SpaceParentEventContent;
use matrix_sdk::ruma::events::{AnySyncStateEvent, StateEventType, SyncStateEvent};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{OwnedRoomId, assign};
use matrix_sdk::{Client, HttpError, Room, RoomState};
use serde::Deserialize;

use super::wire::{PackDefaults, PackDto, ScannedStateDto};
use super::{
    CachedPack, KeyedPacks, MAX_CANONICAL_SPACES, MAX_INFLIGHT_FETCHES, MatrixImagePacks,
    PACK_FRESHNESS, PackFetch, PackRef, PackState, ReferencedPacks,
};
use crate::domain::image_pack::{ImagePack, PackOrigin, PackTitle};

const ROOM_PACK_TYPES: [&str; 2] = ["m.room.image_pack", "im.ponies.room_emotes"];

const PACK_SCAN_CONNECTION: &str = "u2dm-packs";

const ANY_STATE_KEY: &str = "*";

pub(super) const ROOMS_FIELD: &str = "rooms";

pub(super) type ScanCache = StdMutex<HashMap<OwnedRoomId, ScannedRoom>>;

#[derive(Clone)]
pub(super) struct ScannedRoom {
    fetched_at: Instant,
    packs: KeyedPacks,
}

impl MatrixImagePacks {
    pub(super) async fn room_packs(
        &self,
        client: &Client,
        references: Vec<PackRef>,
    ) -> ReferencedPacks {
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

    pub(super) async fn local_packs(
        &self,
        client: &Client,
        rooms: &[OwnedRoomId],
    ) -> ReferencedPacks {
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

pub(super) async fn canonical_spaces(client: &Client, room: &Room) -> Vec<OwnedRoomId> {
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
