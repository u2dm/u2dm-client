use std::collections::HashMap;
use std::mem;
use std::sync::Arc;
use std::time::Duration;

use matrix_sdk::ruma::{OwnedRoomId, RoomId as MatrixRoomId};
use matrix_sdk::sync::RoomUpdates;
use matrix_sdk::{Client, Room};
use matrix_sdk_base::{RoomInfoNotableUpdate, RoomInfoNotableUpdateReasons};
use tokio::time::Instant;

use super::avatars::{AvatarFetcher, AvatarKind};
use super::build::{build_rooms, build_single_room, build_spaces_meta, highest_reported_unread};
use crate::domain::models::{Room as DomainRoom, Space as DomainSpace, SyncEvent};
use crate::ports::matrix::SyncSink as OnSync;

const EMIT_DEBOUNCE: Duration = Duration::from_millis(50);
const UNREAD_HOLDS_STILL_FOR: Duration = Duration::from_millis(1500);
const UNREAD_GIVES_UP_AFTER: Duration = Duration::from_secs(20);

const REBUILD_REASONS: RoomInfoNotableUpdateReasons = RoomInfoNotableUpdateReasons::LATEST_EVENT
    .union(RoomInfoNotableUpdateReasons::MEMBERSHIP)
    .union(RoomInfoNotableUpdateReasons::DISPLAY_NAME)
    .union(RoomInfoNotableUpdateReasons::ACTIVE_SERVICE_MEMBERS)
    .union(RoomInfoNotableUpdateReasons::NONE);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RoomRefresh {
    Counts,
    Full,
}

fn refresh_for(reasons: RoomInfoNotableUpdateReasons) -> Option<RoomRefresh> {
    if reasons.intersects(REBUILD_REASONS) {
        return Some(RoomRefresh::Full);
    }
    if reasons.contains(RoomInfoNotableUpdateReasons::READ_RECEIPT) {
        return Some(RoomRefresh::Counts);
    }
    None
}

#[derive(Clone, Copy)]
struct Counting {
    until: Instant,
    deadline: Instant,
}

#[allow(clippy::struct_excessive_bools)]
pub(super) struct Directory {
    rooms: HashMap<String, Arc<DomainRoom>>,
    order: Vec<String>,
    spaces: Vec<DomainSpace>,
    pending: HashMap<OwnedRoomId, RoomRefresh>,
    rooms_dirty: bool,
    order_dirty: bool,
    spaces_dirty: bool,
    spaces_structural_dirty: bool,
    still_counting: HashMap<String, Counting>,
    flush_at: Option<Instant>,
}

impl Directory {
    pub(super) fn new() -> Self {
        Self {
            rooms: HashMap::new(),
            order: Vec::new(),
            spaces: Vec::new(),
            pending: HashMap::new(),
            rooms_dirty: false,
            order_dirty: false,
            spaces_dirty: false,
            spaces_structural_dirty: false,
            still_counting: HashMap::new(),
            flush_at: None,
        }
    }

    pub(super) fn flush_at(&self) -> Option<Instant> {
        self.flush_at
    }

    fn is_still_counting_unread(&mut self, id: &str, unread: u64, activity: u64) -> bool {
        let now = Instant::now();
        if self.grew_without_a_new_message(id, unread, activity) {
            self.keep_counting(id, now);
        }
        self.counting_until(id, now).is_some()
    }

    fn grew_without_a_new_message(&self, id: &str, unread: u64, activity: u64) -> bool {
        self.rooms.get(id).is_some_and(|previous| {
            unread > previous.unread_count && activity == previous.last_activity_ts
        })
    }

    fn keep_counting(&mut self, id: &str, now: Instant) {
        let counting = self
            .still_counting
            .entry(id.to_owned())
            .or_insert(Counting {
                until: now,
                deadline: now + UNREAD_GIVES_UP_AFTER,
            });
        counting.until = (now + UNREAD_HOLDS_STILL_FOR).min(counting.deadline);
    }

    fn counting_until(&mut self, id: &str, now: Instant) -> Option<Instant> {
        let until = self.still_counting.get(id)?.until;
        if until <= now {
            self.still_counting.remove(id);
            return None;
        }
        Some(until)
    }

    fn settle_finished_counts(&mut self) {
        let now = Instant::now();
        let settled: Vec<String> = self
            .still_counting
            .iter()
            .filter(|(_, counting)| counting.until <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in settled {
            self.still_counting.remove(&id);
            let Some(entry) = self.rooms.get_mut(&id) else {
                continue;
            };
            if !entry.unread_pending {
                continue;
            }
            Arc::make_mut(entry).unread_pending = false;
            self.rooms_dirty = true;
        }
    }

    fn arm_for_counting(&mut self) {
        let Some(next) = self.still_counting.values().map(|c| c.until).min() else {
            return;
        };
        self.flush_at = Some(self.flush_at.map_or(next, |at| at.min(next)));
    }

    fn arm(&mut self) {
        if self.flush_at.is_none() {
            self.flush_at = Some(Instant::now() + EMIT_DEBOUNCE);
        }
    }

    pub(super) fn mark_rooms(&mut self) {
        self.rooms_dirty = true;
        self.arm();
    }

    pub(super) fn mark_spaces(&mut self) {
        self.spaces_dirty = true;
        self.arm();
    }

    fn mark_spaces_structural(&mut self) {
        self.spaces_structural_dirty = true;
        self.arm();
    }

    pub(super) fn mark_kind(&mut self, kind: AvatarKind) {
        match kind {
            AvatarKind::Room => self.mark_rooms(),
            AvatarKind::Space => self.mark_spaces(),
        }
    }

    fn mark_room(&mut self, room_id: OwnedRoomId, refresh: RoomRefresh) {
        let pending = self.pending.entry(room_id).or_insert(refresh);
        *pending = (*pending).max(refresh);
        self.arm();
    }

    pub(super) async fn seed(&mut self, client: &Client) {
        self.rooms = build_rooms(client).await;
        self.spaces = build_spaces_meta(client).await;
        self.pending.clear();
        self.order_dirty = true;
    }

    fn upsert_room(&mut self, mut room: DomainRoom) {
        let key = room.id.to_string();
        room.unread_pending =
            self.is_still_counting_unread(&key, room.unread_count, room.last_activity_ts);
        match self.rooms.get(&key) {
            Some(current) if **current == room => return,
            Some(current) => {
                if current.last_activity_ts != room.last_activity_ts {
                    self.order_dirty = true;
                }
            }
            None => self.order_dirty = true,
        }
        self.rooms.insert(key, Arc::new(room));
        self.mark_rooms();
    }

    fn remove_room(&mut self, room_id: &MatrixRoomId) {
        self.pending.remove(room_id);
        self.still_counting.remove(room_id.as_str());
        if self.rooms.remove(room_id.as_str()).is_none() {
            return;
        }
        self.order_dirty = true;
        self.mark_rooms();
    }

    pub(super) fn note_room_updates(&mut self, client: &Client, updates: &RoomUpdates) {
        for room_id in updates.left.keys() {
            self.remove_room(room_id);
            if self.spaces.iter().any(|space| space.id == room_id.as_str()) {
                self.mark_spaces_structural();
            }
        }
        for room_id in updates.joined.keys() {
            let Some(room) = client.get_room(room_id) else {
                continue;
            };
            if room.is_space() {
                self.mark_spaces_structural();
            } else {
                self.mark_room(room_id.clone(), RoomRefresh::Full);
            }
        }
    }

    pub(super) fn note_room_info(&mut self, client: &Client, update: &RoomInfoNotableUpdate) {
        let Some(refresh) = refresh_for(update.reasons) else {
            return;
        };
        let Some(room) = client.get_room(&update.room_id) else {
            return;
        };
        if room.is_space() {
            self.mark_spaces_structural();
            return;
        }
        if !self.rooms.contains_key(update.room_id.as_str()) {
            return;
        }
        self.mark_room(update.room_id.clone(), refresh);
    }

    async fn apply_pending(&mut self, client: &Client) {
        for (room_id, refresh) in mem::take(&mut self.pending) {
            let Some(room) = client.get_room(&room_id) else {
                continue;
            };
            match refresh {
                RoomRefresh::Full => self.upsert_room(build_single_room(&room).await),
                RoomRefresh::Counts => self.refresh_counts(&room),
            }
        }
    }

    fn refresh_counts(&mut self, room: &Room) {
        let key = room.room_id().as_str();
        if !self.rooms.contains_key(key) {
            return;
        }
        let counts = highest_reported_unread(room);
        let activity = self
            .rooms
            .get(key)
            .map_or(0, |current| current.last_activity_ts);
        let pending = self.is_still_counting_unread(key, counts.unread, activity);
        let Some(entry) = self.rooms.get_mut(key) else {
            return;
        };
        if entry.unread_count == counts.unread
            && entry.mention_count == counts.mentions
            && entry.unread_pending == pending
        {
            return;
        }
        let current = Arc::make_mut(entry);
        current.unread_count = counts.unread;
        current.mention_count = counts.mentions;
        current.unread_pending = pending;
        self.mark_rooms();
    }

    fn refresh_order(&mut self) {
        if !self.order_dirty {
            return;
        }
        self.order = self.rooms.keys().cloned().collect();
        let rooms = &self.rooms;
        self.order.sort_by(|a, b| {
            let activity = |id| {
                rooms
                    .get(id)
                    .map_or(0, |room: &Arc<DomainRoom>| room.last_activity_ts)
            };
            activity(b).cmp(&activity(a)).then_with(|| a.cmp(b))
        });
        self.order_dirty = false;
    }

    pub(super) async fn flush(
        &mut self,
        client: &Client,
        on_sync: &OnSync,
        avatars: &mut AvatarFetcher,
    ) {
        self.settle_finished_counts();
        self.apply_pending(client).await;
        self.flush_at = None;
        if self.spaces_structural_dirty {
            self.spaces = build_spaces_meta(client).await;
            self.spaces_structural_dirty = false;
            self.spaces_dirty = true;
        }
        if self.rooms_dirty {
            self.emit_rooms(client, on_sync, avatars);
            self.rooms_dirty = false;
        }
        if self.spaces_dirty {
            self.emit_spaces(client, on_sync, avatars);
            self.spaces_dirty = false;
        }
        self.arm_for_counting();
    }

    fn emit_rooms(&mut self, client: &Client, on_sync: &OnSync, avatars: &mut AvatarFetcher) {
        self.refresh_order();
        let rooms: Vec<Arc<DomainRoom>> = self
            .order
            .iter()
            .filter_map(|id| self.rooms.get(id))
            .map(Arc::clone)
            .collect();
        avatars.request(
            client,
            AvatarKind::Room,
            rooms.iter().filter_map(|room| room.avatar_mxc.as_deref()),
        );
        on_sync(SyncEvent::Rooms(rooms.into()));
    }

    fn emit_spaces(&self, client: &Client, on_sync: &OnSync, avatars: &mut AvatarFetcher) {
        avatars.request(
            client,
            AvatarKind::Space,
            self.spaces
                .iter()
                .filter_map(|space| space.avatar_mxc.as_deref()),
        );
        on_sync(SyncEvent::Spaces(Arc::from(self.spaces.as_slice())));
    }
}
