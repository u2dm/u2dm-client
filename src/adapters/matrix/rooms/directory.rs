use std::collections::HashMap;
use std::mem;
use std::sync::Arc;
use std::time::Duration;

use matrix_sdk::notification_settings::NotificationSettings;
use matrix_sdk::ruma::{OwnedRoomId, RoomId as MatrixRoomId};
use matrix_sdk::sync::{JoinedRoomUpdate, RoomUpdates, State};
use matrix_sdk::{Client, Room, RoomState};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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

fn refresh_for_joined(update: &JoinedRoomUpdate) -> Option<RoomRefresh> {
    let (State::Before(state) | State::After(state)) = &update.state;
    let rewrites_room =
        !update.timeline.events.is_empty() || !state.is_empty() || !update.account_data.is_empty();
    rewrites_room.then_some(RoomRefresh::Full)
}

fn log_joined_update(
    room_id: &MatrixRoomId,
    update: &JoinedRoomUpdate,
    refresh: Option<RoomRefresh>,
) {
    let (State::Before(state) | State::After(state)) = &update.state;
    tracing::debug!(
        %room_id,
        events = update.timeline.events.len(),
        limited = update.timeline.limited,
        state = state.len(),
        account_data = update.account_data.len(),
        ephemeral = update.ephemeral.len(),
        notifications = update.unread_notifications.notification_count,
        highlights = update.unread_notifications.highlight_count,
        ?refresh,
        "sync updated the room"
    );
}

fn changed_facets(before: &DomainRoom, after: &DomainRoom) -> Vec<&'static str> {
    let DomainRoom {
        id: _,
        display_name,
        avatar_mxc,
        topic,
        canonical_alias,
        is_direct,
        is_encrypted,
        poll_permissions,
        message_permissions,
        member_count,
        has_unread,
        has_mentions,
        has_activity,
        notify,
        last_activity_ts,
        last_message_sender,
        last_message_kind,
        last_message_body,
        last_message_service,
        last_message_is_own,
        last_message_edited,
    } = after;
    let preview_changed = before.last_message_sender != *last_message_sender
        || before.last_message_kind != *last_message_kind
        || before.last_message_body != *last_message_body
        || before.last_message_service != *last_message_service
        || before.last_message_is_own != *last_message_is_own
        || before.last_message_edited != *last_message_edited;
    let unread_changed = before.has_unread != *has_unread
        || before.has_mentions != *has_mentions
        || before.has_activity != *has_activity;
    [
        ("name", before.display_name != *display_name),
        ("avatar", before.avatar_mxc != *avatar_mxc),
        ("topic", before.topic != *topic),
        ("alias", before.canonical_alias != *canonical_alias),
        ("direct", before.is_direct != *is_direct),
        ("encryption", before.is_encrypted != *is_encrypted),
        (
            "poll permissions",
            before.poll_permissions != *poll_permissions,
        ),
        (
            "message permissions",
            before.message_permissions != *message_permissions,
        ),
        ("members", before.member_count != *member_count),
        ("unread", unread_changed),
        ("notifications", before.notify != *notify),
        ("activity", before.last_activity_ts != *last_activity_ts),
        ("preview", preview_changed),
    ]
    .into_iter()
    .filter_map(|(facet, changed)| changed.then_some(facet))
    .collect()
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

    fn remove_room(&mut self, room_id: &MatrixRoomId) {
        self.pending.remove(room_id);
        if self.rooms.remove(room_id.as_str()).is_none() {
            return;
        }
        tracing::debug!(%room_id, "dropped the room from the list");
        self.order_dirty = true;
        self.mark_rooms();
    }

    pub(super) fn note_room_updates(&mut self, client: &Client, updates: &RoomUpdates) {
        for room_id in updates.left.keys() {
            tracing::debug!(%room_id, "sync reported the room as left");
            self.remove_room(room_id);
            if self.spaces.iter().any(|space| space.id == room_id.as_str()) {
                self.mark_spaces_structural();
            }
        }
        for (room_id, update) in &updates.joined {
            let refresh = refresh_for_joined(update);
            log_joined_update(room_id, update, refresh);
            let Some(refresh) = refresh else {
                continue;
            };
            let Some(room) = client.get_room(room_id) else {
                tracing::debug!(%room_id, "the client does not know the room sync updated");
                continue;
            };
            if room.is_space() {
                self.mark_spaces_structural();
            } else {
                self.mark_room(room_id.clone(), refresh);
            }
        }
    }

    pub(super) fn note_room_info(&mut self, client: &Client, update: &RoomInfoNotableUpdate) {
        let refresh = refresh_for(update.reasons);
        tracing::debug!(
            room_id = %update.room_id,
            reasons = ?update.reasons,
            ?refresh,
            "the room info changed"
        );
        let Some(refresh) = refresh else {
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
            tracing::debug!(
                room_id = %update.room_id,
                "the room is not listed yet, so it waits for sync to list it"
            );
            return;
        }
        self.mark_room(update.room_id.clone(), refresh);
    }

    async fn apply_pending(&mut self, client: &Client) {
        for (room_id, refresh) in mem::take(&mut self.pending) {
            self.refresh_room(client, &room_id, refresh).await;
        }
    }

    async fn refresh_room(
        &mut self,
        client: &Client,
        room_id: &MatrixRoomId,
        refresh: RoomRefresh,
    ) {
        let Some(room) = client.get_room(room_id) else {
            tracing::debug!(%room_id, ?refresh, "the client no longer knows the room");
            return;
        };
        let state = room.state();
        if state != RoomState::Joined {
            tracing::debug!(%room_id, ?state, "the room is no longer joined");
            self.remove_room(room_id);
            return;
        }
        tracing::debug!(%room_id, ?refresh, "refreshing the room");
        match refresh {
            RoomRefresh::Full => {
                let built = build_single_room(&room, &self.notifications).await;
                self.upsert_room(built);
            }
            RoomRefresh::Flags => self.refresh_flags(&room).await,
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
            tracing::debug!(room_id = key, "the unread flags did not change");
            return;
        }
        tracing::debug!(
            room_id = key,
            unread = flags.has_unread,
            mentions = flags.has_mentions,
            activity = flags.has_activity,
            notify = ?flags.notify,
            "the unread flags changed"
        );
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
