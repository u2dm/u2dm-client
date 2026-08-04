use std::collections::HashMap;
use std::mem;
use std::sync::Arc;
use std::time::Duration;

use matrix_sdk::notification_settings::NotificationSettings;
use matrix_sdk::ruma::{OwnedRoomId, RoomId as MatrixRoomId};
use matrix_sdk::sync::RoomUpdates;
use matrix_sdk::{Client, Room};
use matrix_sdk_base::{RoomInfoNotableUpdate, RoomInfoNotableUpdateReasons};
use tokio::time::Instant;

use super::avatars::{AvatarFetcher, AvatarKind};
use super::build::{build_rooms, build_single_room, build_spaces_meta, unread_flags};
use crate::domain::room::{Room as DomainRoom, Space as DomainSpace};
use crate::domain::sync::SyncEvent;
use crate::ports::matrix::SyncSink as OnSync;

const EMIT_DEBOUNCE: Duration = Duration::from_millis(50);

const REBUILD_REASONS: RoomInfoNotableUpdateReasons = RoomInfoNotableUpdateReasons::LATEST_EVENT
    .union(RoomInfoNotableUpdateReasons::MEMBERSHIP)
    .union(RoomInfoNotableUpdateReasons::DISPLAY_NAME)
    .union(RoomInfoNotableUpdateReasons::ACTIVE_SERVICE_MEMBERS)
    .union(RoomInfoNotableUpdateReasons::NONE);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RoomRefresh {
    Flags,
    Full,
}

fn refresh_for(reasons: RoomInfoNotableUpdateReasons) -> Option<RoomRefresh> {
    if reasons.intersects(REBUILD_REASONS) {
        return Some(RoomRefresh::Full);
    }
    if reasons.contains(RoomInfoNotableUpdateReasons::READ_RECEIPT) {
        return Some(RoomRefresh::Flags);
    }
    None
}

#[allow(clippy::struct_excessive_bools)]
pub(super) struct Directory {
    rooms: HashMap<String, Arc<DomainRoom>>,
    order: Vec<String>,
    spaces: Vec<DomainSpace>,
    pending: HashMap<OwnedRoomId, RoomRefresh>,
    notifications: NotificationSettings,
    rooms_dirty: bool,
    order_dirty: bool,
    spaces_dirty: bool,
    spaces_structural_dirty: bool,
    flush_at: Option<Instant>,
}

impl Directory {
    pub(super) fn new(notifications: NotificationSettings) -> Self {
        Self {
            rooms: HashMap::new(),
            order: Vec::new(),
            spaces: Vec::new(),
            pending: HashMap::new(),
            notifications,
            rooms_dirty: false,
            order_dirty: false,
            spaces_dirty: false,
            spaces_structural_dirty: false,
            flush_at: None,
        }
    }

    pub(super) fn flush_at(&self) -> Option<Instant> {
        self.flush_at
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

    pub(super) fn mark_all_flags(&mut self) {
        let ids: Vec<OwnedRoomId> = self
            .rooms
            .keys()
            .filter_map(|id| MatrixRoomId::parse(id).ok())
            .collect();
        for id in ids {
            self.mark_room(id, RoomRefresh::Flags);
        }
    }

    pub(super) async fn seed(&mut self, client: &Client) {
        let rooms = build_rooms(client, &self.notifications).await;
        self.rooms = rooms;
        self.spaces = build_spaces_meta(client).await;
        self.pending.clear();
        self.order_dirty = true;
    }

    fn upsert_room(&mut self, room: DomainRoom) {
        let key = room.id.to_string();
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
                RoomRefresh::Full => {
                    let built = build_single_room(&room, &self.notifications).await;
                    self.upsert_room(built);
                }
                RoomRefresh::Flags => self.refresh_flags(&room).await,
            }
        }
    }

    async fn refresh_flags(&mut self, room: &Room) {
        let key = room.room_id().as_str();
        if !self.rooms.contains_key(key) {
            return;
        }
        let flags = unread_flags(room, &self.notifications).await;
        let Some(entry) = self.rooms.get_mut(key) else {
            return;
        };
        if entry.has_unread == flags.has_unread
            && entry.has_mentions == flags.has_mentions
            && entry.has_activity == flags.has_activity
            && entry.notify == flags.notify
        {
            return;
        }
        let current = Arc::make_mut(entry);
        current.has_unread = flags.has_unread;
        current.has_mentions = flags.has_mentions;
        current.has_activity = flags.has_activity;
        current.notify = flags.notify;
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
