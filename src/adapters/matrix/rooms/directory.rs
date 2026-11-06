mod refresh;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use matrix_sdk::Client;
use matrix_sdk::notification_settings::NotificationSettings;
use matrix_sdk::ruma::{OwnedRoomId, RoomId as MatrixRoomId};
use tokio::time::Instant;

use self::refresh::{RoomRefresh, changed_facets};
use super::avatars::{AvatarFetcher, AvatarKind};
use super::build::{build_rooms, build_spaces_meta};
use super::invites::build_invites;
use super::removals::build_removals;
use crate::domain::departure::Removal;
use crate::domain::invite::Invite;
use crate::domain::room::{Room as DomainRoom, Space as DomainSpace};
use crate::domain::sync::SyncEvent;
use crate::ports::matrix::SyncSink as OnSync;
use crate::util::unix_now_ms;

const EMIT_DEBOUNCE: Duration = Duration::from_millis(50);

const CALL_EXPIRY_SLACK_MS: u64 = 1_000;

#[allow(clippy::struct_excessive_bools)]
pub(super) struct Directory {
    rooms: HashMap<String, Arc<DomainRoom>>,
    order: Vec<String>,
    invites: HashMap<String, Arc<Invite>>,
    removals: HashMap<String, Arc<Removal>>,
    spaces: Vec<DomainSpace>,
    pending: HashMap<OwnedRoomId, RoomRefresh>,
    notifications: NotificationSettings,
    rooms_dirty: bool,
    order_dirty: bool,
    invites_dirty: bool,
    removals_dirty: bool,
    spaces_dirty: bool,
    spaces_structural_dirty: bool,
    flush_at: Option<Instant>,
}

impl Directory {
    pub(super) fn new(notifications: NotificationSettings) -> Self {
        Self {
            rooms: HashMap::new(),
            order: Vec::new(),
            invites: HashMap::new(),
            removals: HashMap::new(),
            spaces: Vec::new(),
            pending: HashMap::new(),
            notifications,
            rooms_dirty: false,
            order_dirty: false,
            invites_dirty: false,
            removals_dirty: false,
            spaces_dirty: false,
            spaces_structural_dirty: false,
            flush_at: None,
        }
    }

    pub(super) fn wake_at(&self) -> Option<Instant> {
        match (self.flush_at, self.call_expiry_at()) {
            (Some(flush), Some(expiry)) => Some(flush.min(expiry)),
            (flush, expiry) => flush.or(expiry),
        }
    }

    fn call_expiry_at(&self) -> Option<Instant> {
        let next = self
            .rooms
            .values()
            .filter_map(|room| room.call.as_ref()?.next_expiry_ms())
            .min()?;
        let due = next.saturating_add(CALL_EXPIRY_SLACK_MS);
        Some(Instant::now() + Duration::from_millis(due.saturating_sub(unix_now_ms())))
    }

    pub(super) fn expire_calls(&mut self) {
        let now = unix_now_ms();
        let expired: Vec<OwnedRoomId> = self
            .rooms
            .values()
            .filter(|room| {
                room.call
                    .as_ref()
                    .and_then(|call| call.next_expiry_ms())
                    .is_some_and(|expiry| expiry <= now)
            })
            .filter_map(|room| MatrixRoomId::parse(room.id.as_ref()).ok())
            .collect();
        for room_id in expired {
            tracing::debug!(%room_id, "a call membership expired");
            self.mark_room(room_id, RoomRefresh::Full);
        }
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

    pub(super) fn mark_invites(&mut self) {
        self.invites_dirty = true;
        self.arm();
    }

    pub(super) fn mark_removals(&mut self) {
        self.removals_dirty = true;
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
            AvatarKind::Invite => self.mark_invites(),
            AvatarKind::Removal => self.mark_removals(),
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
        self.invites = build_invites(client).await;
        self.removals = build_removals(client).await;
        self.spaces = build_spaces_meta(client).await;
        self.pending.clear();
        self.order_dirty = true;
    }

    fn upsert_room(&mut self, room: DomainRoom) {
        let key = room.id.to_string();
        match self.rooms.get(&key) {
            Some(current) if **current == room => {
                tracing::debug!(room_id = %room.id, "the rebuilt room shows nothing new");
                return;
            }
            Some(current) => {
                tracing::debug!(
                    room_id = %room.id,
                    changed = ?changed_facets(current, &room),
                    "the room changed"
                );
                if current.last_activity_ts != room.last_activity_ts {
                    self.order_dirty = true;
                }
            }
            None => {
                tracing::debug!(room_id = %room.id, "listing the room");
                self.order_dirty = true;
            }
        }
        self.rooms.insert(key, Arc::new(room));
        self.mark_rooms();
    }

    fn upsert_invite(&mut self, invite: Invite) {
        let key = invite.id.to_string();
        if self
            .invites
            .get(&key)
            .is_some_and(|current| **current == invite)
        {
            tracing::debug!(room_id = %invite.id, "the rebuilt invite shows nothing new");
            return;
        }
        tracing::debug!(room_id = %invite.id, kind = ?invite.kind, "listing the invite");
        self.invites.insert(key, Arc::new(invite));
        self.mark_invites();
    }

    fn upsert_removal(&mut self, removal: Removal) {
        let key = removal.id.to_string();
        if self
            .removals
            .get(&key)
            .is_some_and(|current| **current == removal)
        {
            tracing::debug!(room_id = %removal.id, "the rebuilt removal shows nothing new");
            return;
        }
        tracing::info!(room_id = %removal.id, kind = ?removal.kind, "listing the room you were removed from");
        self.removals.insert(key, Arc::new(removal));
        self.mark_removals();
    }

    fn remove_room(&mut self, room_id: &MatrixRoomId) {
        self.pending.remove(room_id);
        self.unlist_room(room_id);
        self.unlist_invite(room_id);
        self.unlist_removal(room_id);
    }

    fn unlist_room(&mut self, room_id: &MatrixRoomId) {
        if self.rooms.remove(room_id.as_str()).is_none() {
            return;
        }
        tracing::debug!(%room_id, "dropped the room from the list");
        self.order_dirty = true;
        self.mark_rooms();
    }

    fn unlist_invite(&mut self, room_id: &MatrixRoomId) {
        if self.invites.remove(room_id.as_str()).is_none() {
            return;
        }
        tracing::debug!(%room_id, "dropped the invite from the list");
        self.mark_invites();
    }

    fn unlist_removal(&mut self, room_id: &MatrixRoomId) {
        if self.removals.remove(room_id.as_str()).is_none() {
            return;
        }
        tracing::debug!(%room_id, "dropped the removal from the list");
        self.mark_removals();
    }

    fn lists(&self, room_id: &MatrixRoomId) -> bool {
        self.rooms.contains_key(room_id.as_str())
            || self.invites.contains_key(room_id.as_str())
            || self.removals.contains_key(room_id.as_str())
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
        if self.removals_dirty {
            self.emit_removals(client, on_sync, avatars);
            self.removals_dirty = false;
        }
        if self.rooms_dirty {
            self.emit_rooms(client, on_sync, avatars);
            self.rooms_dirty = false;
        }
        if self.invites_dirty {
            self.emit_invites(client, on_sync, avatars);
            self.invites_dirty = false;
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

    fn emit_invites(&self, client: &Client, on_sync: &OnSync, avatars: &mut AvatarFetcher) {
        let mut invites: Vec<Arc<Invite>> = self.invites.values().map(Arc::clone).collect();
        invites.sort_by(|a, b| {
            b.received_ts
                .cmp(&a.received_ts)
                .then_with(|| a.id.as_ref().cmp(b.id.as_ref()))
        });
        avatars.request(
            client,
            AvatarKind::Invite,
            invites
                .iter()
                .filter_map(|invite| invite.avatar_mxc.as_deref()),
        );
        on_sync(SyncEvent::Invites(invites.into()));
    }

    fn emit_removals(&mut self, client: &Client, on_sync: &OnSync, avatars: &mut AvatarFetcher) {
        self.removals.retain(|room_id, _| {
            let known = MatrixRoomId::parse(room_id)
                .ok()
                .and_then(|id| client.get_room(&id))
                .is_some();
            if !known {
                tracing::debug!(room_id, "the client forgot the room you were removed from");
            }
            known
        });
        let mut removals: Vec<Arc<Removal>> = self.removals.values().map(Arc::clone).collect();
        removals.sort_by(|a, b| {
            b.removed_ts
                .cmp(&a.removed_ts)
                .then_with(|| a.id.as_ref().cmp(b.id.as_ref()))
        });
        avatars.request(
            client,
            AvatarKind::Removal,
            removals
                .iter()
                .filter_map(|removal| removal.avatar_mxc.as_deref()),
        );
        on_sync(SyncEvent::Removals(removals.into()));
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
