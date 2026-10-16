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
use tracing::Instrument;

use super::avatars::{AvatarFetcher, AvatarKind};
use super::build::{build_rooms, build_single_room, build_spaces_meta, unread_flags};
use super::invites::{build_invite, build_invites};
use crate::domain::invite::Invite;
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
        direct_partner,
        is_encrypted,
        history_visibility,
        poll_permissions,
        message_permissions,
        may_invite,
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
        successor,
        predecessor,
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
        ("partner", before.direct_partner != *direct_partner),
        ("encryption", before.is_encrypted != *is_encrypted),
        ("history", before.history_visibility != *history_visibility),
        (
            "poll permissions",
            before.poll_permissions != *poll_permissions,
        ),
        (
            "message permissions",
            before.message_permissions != *message_permissions,
        ),
        ("invite permission", before.may_invite != *may_invite),
        ("members", before.member_count != *member_count),
        ("unread", unread_changed),
        ("notifications", before.notify != *notify),
        ("activity", before.last_activity_ts != *last_activity_ts),
        ("preview", preview_changed),
        (
            "upgrade",
            before.successor != *successor || before.predecessor != *predecessor,
        ),
    ]
    .into_iter()
    .filter_map(|(facet, changed)| changed.then_some(facet))
    .collect()
}

#[allow(clippy::struct_excessive_bools)]
pub(super) struct Directory {
    rooms: HashMap<String, Arc<DomainRoom>>,
    order: Vec<String>,
    invites: HashMap<String, Arc<Invite>>,
    spaces: Vec<DomainSpace>,
    pending: HashMap<OwnedRoomId, RoomRefresh>,
    notifications: NotificationSettings,
    rooms_dirty: bool,
    order_dirty: bool,
    invites_dirty: bool,
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
            spaces: Vec::new(),
            pending: HashMap::new(),
            notifications,
            rooms_dirty: false,
            order_dirty: false,
            invites_dirty: false,
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

    pub(super) fn mark_invites(&mut self) {
        self.invites_dirty = true;
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

    fn remove_room(&mut self, room_id: &MatrixRoomId) {
        self.pending.remove(room_id);
        self.unlist_room(room_id);
        self.unlist_invite(room_id);
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

    fn lists(&self, room_id: &MatrixRoomId) -> bool {
        self.rooms.contains_key(room_id.as_str()) || self.invites.contains_key(room_id.as_str())
    }

    pub(super) fn note_room_updates(&mut self, client: &Client, updates: &RoomUpdates) {
        for room_id in updates.left.keys() {
            self.note_left(room_id);
        }
        for room_id in updates.invited.keys() {
            tracing::debug!(%room_id, "sync delivered an invite");
            self.mark_room(room_id.clone(), RoomRefresh::Full);
        }
        for (room_id, update) in &updates.joined {
            self.note_joined(client, room_id, update);
        }
    }

    fn note_left(&mut self, room_id: &MatrixRoomId) {
        tracing::debug!(%room_id, "sync reported the room as left");
        self.remove_room(room_id);
        if self.spaces.iter().any(|space| space.id == room_id.as_str()) {
            self.mark_spaces_structural();
        }
    }

    fn note_joined(&mut self, client: &Client, room_id: &OwnedRoomId, update: &JoinedRoomUpdate) {
        let refresh = refresh_for_joined(update);
        log_joined_update(room_id, update, refresh);
        let was_invited = self.invites.contains_key(room_id.as_str());
        let Some(refresh) = refresh.or(was_invited.then_some(RoomRefresh::Full)) else {
            return;
        };
        let Some(room) = client.get_room(room_id) else {
            tracing::debug!(%room_id, "the client does not know the room sync updated");
            return;
        };
        if room.is_space() {
            self.mark_spaces_structural();
        }
        if !room.is_space() || was_invited {
            self.mark_room(room_id.clone(), refresh);
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
        let room_id = &update.room_id;
        let invited = self.invites.contains_key(room_id.as_str());
        let Some(room) = client.get_room(room_id) else {
            if self.lists(room_id) {
                self.mark_room(room_id.clone(), RoomRefresh::Full);
            }
            return;
        };
        if room.is_space() && room.state() != RoomState::Invited {
            self.mark_spaces_structural();
        }
        if invited {
            self.mark_room(room_id.clone(), RoomRefresh::Full);
            return;
        }
        if room.is_space() {
            return;
        }
        if !self.rooms.contains_key(room_id.as_str()) {
            tracing::debug!(%room_id, "the room is not listed yet, so it waits for sync to list it");
            return;
        }
        self.mark_room(room_id.clone(), refresh);
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
            self.remove_room(room_id);
            return;
        };
        match room.state() {
            RoomState::Invited => {
                self.unlist_room(room_id);
                let invite = build_invite(&room)
                    .instrument(tracing::debug_span!("invite_build", %room_id))
                    .await;
                self.upsert_invite(invite);
            }
            RoomState::Joined => {
                self.unlist_invite(room_id);
                if !room.is_space() {
                    self.refresh_joined(&room, refresh).await;
                }
            }
            state => {
                tracing::debug!(%room_id, ?state, "the room is no longer joined or invited");
                self.remove_room(room_id);
            }
        }
    }

    async fn refresh_joined(&mut self, room: &Room, refresh: RoomRefresh) {
        let listed = self.rooms.contains_key(room.room_id().as_str());
        tracing::debug!(room_id = %room.room_id(), ?refresh, listed, "refreshing the room");
        if refresh == RoomRefresh::Flags && listed {
            self.refresh_flags(room).await;
            return;
        }
        let built = build_single_room(room, &self.notifications).await;
        self.upsert_room(built);
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
