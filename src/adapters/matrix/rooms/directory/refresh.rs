use std::mem;
use std::sync::Arc;

use matrix_sdk::ruma::{OwnedRoomId, RoomId as MatrixRoomId};
use matrix_sdk::sync::{JoinedRoomUpdate, RoomUpdates, State};
use matrix_sdk::{Client, Room, RoomState};
use matrix_sdk_base::{RoomInfoNotableUpdate, RoomInfoNotableUpdateReasons};
use tracing::Instrument;

use super::Directory;
use crate::adapters::matrix::rooms::build::{build_single_room, unread_flags};
use crate::adapters::matrix::rooms::invites::build_invite;
use crate::adapters::matrix::rooms::removals::build_removal;
use crate::domain::room::Room as DomainRoom;

const REBUILD_REASONS: RoomInfoNotableUpdateReasons = RoomInfoNotableUpdateReasons::LATEST_EVENT
    .union(RoomInfoNotableUpdateReasons::MEMBERSHIP)
    .union(RoomInfoNotableUpdateReasons::DISPLAY_NAME)
    .union(RoomInfoNotableUpdateReasons::ACTIVE_SERVICE_MEMBERS)
    .union(RoomInfoNotableUpdateReasons::NONE);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum RoomRefresh {
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

pub(super) fn changed_facets(before: &DomainRoom, after: &DomainRoom) -> Vec<&'static str> {
    let DomainRoom {
        id: _,
        display_name,
        avatar_mxc,
        topic,
        canonical_alias,
        is_direct,
        direct_partner,
        is_encrypted,
        event_ids,
        history_visibility,
        poll_permissions,
        message_permissions,
        may_invite,
        may_join_calls,
        call,
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
        ("version", before.event_ids != *event_ids),
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
        ("call permission", before.may_join_calls != *may_join_calls),
        ("call", before.call != *call),
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

impl Directory {
    pub(in crate::adapters::matrix::rooms) fn note_room_updates(
        &mut self,
        client: &Client,
        updates: &RoomUpdates,
    ) {
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

    fn note_left(&mut self, room_id: &OwnedRoomId) {
        tracing::debug!(%room_id, "sync reported the room as left");
        self.mark_room(room_id.clone(), RoomRefresh::Full);
        if self.spaces.iter().any(|space| space.id == room_id.as_str()) {
            self.mark_spaces_structural();
        }
    }

    fn note_joined(&mut self, client: &Client, room_id: &OwnedRoomId, update: &JoinedRoomUpdate) {
        let refresh = refresh_for_joined(update);
        log_joined_update(room_id, update, refresh);
        let was_invited = self.invites.contains_key(room_id.as_str())
            || self.removals.contains_key(room_id.as_str());
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

    pub(in crate::adapters::matrix::rooms) fn note_room_info(
        &mut self,
        client: &Client,
        update: &RoomInfoNotableUpdate,
    ) {
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
        let invited = self.invites.contains_key(room_id.as_str())
            || self.removals.contains_key(room_id.as_str());
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

    pub(super) async fn apply_pending(&mut self, client: &Client) {
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
                self.unlist_removal(room_id);
                let invite = build_invite(&room)
                    .instrument(tracing::debug_span!("invite_build", %room_id))
                    .await;
                self.upsert_invite(invite);
            }
            RoomState::Joined => {
                self.unlist_invite(room_id);
                self.unlist_removal(room_id);
                if !room.is_space() {
                    self.refresh_joined(&room, refresh).await;
                }
            }
            RoomState::Left | RoomState::Banned => self.refresh_departed(&room).await,
            RoomState::Knocked => {
                tracing::debug!(%room_id, "the room is only knocked on");
                self.remove_room(room_id);
            }
        }
    }

    async fn refresh_departed(&mut self, room: &Room) {
        let room_id = room.room_id();
        let removal = build_removal(room)
            .instrument(tracing::debug_span!("removal_build", %room_id))
            .await;
        let Some(removal) = removal else {
            tracing::debug!(%room_id, state = ?room.state(), "the room is no longer joined or invited");
            self.remove_room(room_id);
            return;
        };
        self.unlist_room(room_id);
        self.unlist_invite(room_id);
        self.upsert_removal(removal);
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

    pub(super) fn refresh_order(&mut self) {
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
}
