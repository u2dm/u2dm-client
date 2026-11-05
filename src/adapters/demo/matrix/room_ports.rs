use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::{DemoAuthed, RoomLists, current_lists, unavailable};
use crate::adapters::demo::{
    account, calls, create_room, data, departures, invites, links, media, notifications, room_info,
    space_index, sync, timeline, upgrades,
};
use crate::domain::departure::AdminStanding;
use crate::domain::matrix_link::RoomAddress;
use crate::domain::member_name::NamedUser;
use crate::domain::push::{NoticeContent, PushNotice, PushTweaks};
use crate::domain::room::RoomId;
use crate::domain::room_creation::{
    ChildLink, CreateFailure, CreateResult, CreatedRoom, JoinAccess, RoomBlueprint, RoomKind, Shape,
};
use crate::domain::room_preview::{PreviewFailure, PreviewMembership, PreviewResult};
use crate::domain::space_index::HierarchyPage;
use crate::domain::sync::{SyncEvent, SyncOutcome, SyncSupport};
use crate::domain::user_info::server_part;
use crate::error::{AppError, Result};
use crate::ports::matrix::{
    InvitePort, LeavePort, PushPort, RoomCreationPort, RoomPreviewPort, SpaceIndexPort,
    SpaceOrderPort, SyncPort, SyncSink, UpgradePort,
};
use crate::util::unix_now_ms;

#[async_trait]
impl SyncPort for DemoAuthed {
    async fn start_sync(&self, on_sync: SyncSink, cancel: CancellationToken) -> SyncOutcome {
        if let Ok(mut sink) = self.sync_sink.lock() {
            *sink = Some(Arc::clone(&on_sync));
        }
        self.hold_back_late_invites();
        self.keep_stored_removals();
        calls::book().begin();
        let lists = current_lists(&self.lists);
        on_sync(SyncEvent::Removals(lists.removals().into()));
        on_sync(SyncEvent::Rooms(lists.rooms().into()));
        on_sync(SyncEvent::Invites(lists.invites().into()));
        on_sync(SyncEvent::Spaces(lists.spaces().into()));
        if sync::support() == SyncSupport::ClassicOnly {
            return sync::refuse_forever(&on_sync, &cancel).await;
        }
        on_sync(SyncEvent::Connected);
        self.spawn_invite_changes();
        self.spawn_live_removal();
        self.spawn_call_script();
        if !timeline::scenario().room_list_keeps_updating {
            cancel.cancelled().await;
            return SyncOutcome::Cancelled;
        }
        loop {
            tokio::select! {
                () = cancel.cancelled() => return SyncOutcome::Cancelled,
                () = sleep(timeline::ROOM_LIST_INTERVAL) => {
                    let lists = current_lists(&self.lists);
                    on_sync(SyncEvent::Removals(lists.removals().into()));
                    on_sync(SyncEvent::Rooms(lists.rooms().into()));
                    on_sync(SyncEvent::Invites(lists.invites().into()));
                }
            }
        }
    }

    fn set_selected_room(&self, room_id: Option<&RoomId>) {
        tracing::debug!(?room_id, "demo: the selected room needs no subscription");
    }

    fn set_call_room(&self, room_id: Option<&RoomId>) {
        tracing::debug!(?room_id, "demo: the call's room needs no subscription");
    }
}

impl DemoAuthed {
    pub(super) fn sync_sink(&self) -> Option<SyncSink> {
        self.sync_sink.lock().ok().and_then(|sink| sink.clone())
    }

    pub(super) fn change_lists(&self, change: impl FnOnce(&mut RoomLists)) {
        if let Ok(mut lists) = self.lists.lock() {
            change(&mut lists);
        }
    }

    fn remember_join(&self, room_id: &RoomId) {
        self.change_lists(|lists| {
            if !lists.joined.contains(room_id) {
                lists.joined.push(room_id.clone());
            }
            lists.overrides.left.remove(room_id.as_ref());
        });
    }

    fn echo_join(&self) {
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(space_index::JOIN_ECHO_LAG).await;
            let lists = current_lists(&lists);
            sink(SyncEvent::Removals(lists.removals().into()));
            sink(SyncEvent::Rooms(lists.rooms().into()));
            sink(SyncEvent::Invites(lists.invites().into()));
            sink(SyncEvent::Spaces(lists.spaces().into()));
        });
    }

    fn echo_departure(&self) {
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(room_info::ECHO_LAG).await;
            let lists = current_lists(&lists);
            sink(SyncEvent::Removals(lists.removals().into()));
            sink(SyncEvent::Rooms(lists.rooms().into()));
            sink(SyncEvent::Invites(lists.invites().into()));
            sink(SyncEvent::Spaces(lists.spaces().into()));
        });
    }

    fn keep_stored_removals(&self) {
        if !departures::scenario().stored {
            return;
        }
        self.change_lists(|lists| {
            for stored in departures::STORED_REMOVALS {
                lists
                    .joined
                    .retain(|joined| joined.as_ref() != stored.room_id);
                lists.overrides.left.insert(stored.room_id.to_owned());
                lists.overrides.removed.insert(
                    stored.room_id.to_owned(),
                    data::Removed::ago(stored.kind, stored.by, stored.reason, stored.minutes_ago),
                );
            }
        });
    }

    fn spawn_live_removal(&self) {
        let Some(kind) = departures::scenario().live_removal() else {
            return;
        };
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(departures::KICK_AFTER).await;
            if let Ok(mut lists) = lists.lock() {
                let room_id = departures::KICKED_ROOM;
                lists.joined.retain(|joined| joined.as_ref() != room_id);
                lists.overrides.left.insert(room_id.to_owned());
                lists.overrides.forgotten.remove(room_id);
                lists.overrides.removed.insert(
                    room_id.to_owned(),
                    data::Removed::now(
                        kind,
                        departures::KICKED_BY,
                        Some(departures::KICKED_REASON),
                    ),
                );
            }
            tracing::info!(
                room_id = departures::KICKED_ROOM,
                ?kind,
                "demo: you were removed from a room"
            );
            let lists = current_lists(&lists);
            sink(SyncEvent::Removals(lists.removals().into()));
            sink(SyncEvent::Rooms(lists.rooms().into()));
        });
    }

    fn hold_back_late_invites(&self) {
        if invites::scenario().arrives_late {
            self.change_lists(|lists| {
                lists
                    .overrides
                    .held_back
                    .insert(invites::LATE_INVITE.to_owned());
            });
        }
        if notifications::scenario().invite {
            self.change_lists(|lists| {
                lists
                    .overrides
                    .held_back
                    .insert(notifications::LATE_INVITE.to_owned());
            });
        }
    }

    fn emit_rooms(&self) {
        if let Some(sink) = self.sync_sink() {
            sink(SyncEvent::Rooms(current_lists(&self.lists).rooms().into()));
        }
    }

    async fn arrive(
        &self,
        arrival: &notifications::Arrival,
        notices_tx: &mpsc::Sender<PushNotice>,
    ) -> bool {
        let Some(room) = current_lists(&self.lists)
            .rooms()
            .into_iter()
            .find(|room| room.id == arrival.room_id)
        else {
            tracing::debug!(room_id = %arrival.room_id, "demo: an arrival names no listed room");
            return true;
        };
        let sender = data::newest_other_sender(&arrival.room_id)
            .unwrap_or_else(|| NamedUser::unnamed(room.id.to_string()));
        let message = data::arrived_message(
            self.sent.fetch_add(1, Ordering::Relaxed),
            &sender,
            arrival.body.clone(),
        );
        if let Ok(mut arrived) = self.arrived.lock() {
            arrived
                .entry(arrival.room_id.clone())
                .or_default()
                .push(message);
        }
        let shown_sender = (!room.is_direct).then_some(sender);
        if arrival.notifies {
            tracing::debug!(room_id = %arrival.room_id, "demo: push rules notify an arrival");
            let notice = PushNotice {
                room_id: arrival.room_id.clone(),
                room_name: room.display_name.clone(),
                sent_at: data::arrived_at(),
                content: NoticeContent::Message {
                    sender: shown_sender.clone(),
                    kind: arrival.kind,
                    body: arrival.preview(),
                },
                tweaks: arrival.tweaks,
            };
            if notices_tx.send(notice).await.is_err() {
                return false;
            }
        }
        sleep(notifications::ROOM_ECHO_LAG).await;
        let arrived = data::Arrived::now(
            shown_sender,
            arrival.kind,
            arrival.preview(),
            arrival.tweaks.highlight,
        );
        self.change_lists(|lists| {
            lists.overrides.read.remove(arrival.room_id.as_ref());
            lists
                .overrides
                .arrivals
                .insert(arrival.room_id.to_string(), arrived);
        });
        self.emit_rooms();
        true
    }

    fn read_elsewhere(&self, room_id: &RoomId) {
        tracing::debug!(%room_id, "demo: a room is read on another device");
        self.change_lists(|lists| {
            lists.overrides.read.insert(room_id.to_string());
        });
        self.emit_rooms();
    }

    async fn release_notified_invite(&self, notices_tx: &mpsc::Sender<PushNotice>) -> bool {
        self.change_lists(|lists| {
            lists.overrides.held_back.remove(notifications::LATE_INVITE);
        });
        let lists = current_lists(&self.lists);
        let Some(invite) = lists
            .invites()
            .into_iter()
            .find(|invite| invite.id.as_ref() == notifications::LATE_INVITE)
        else {
            return true;
        };
        let notice = PushNotice {
            room_id: invite.id.clone(),
            room_name: invite.name.clone(),
            sent_at: data::arrived_at(),
            content: NoticeContent::Invite {
                inviter: invite.inviter.clone(),
            },
            tweaks: PushTweaks {
                highlight: false,
                sound: true,
            },
        };
        tracing::debug!(room_id = %invite.id, "demo: push rules notify an invite");
        if notices_tx.send(notice).await.is_err() {
            return false;
        }
        sleep(notifications::ROOM_ECHO_LAG).await;
        if let Some(sink) = self.sync_sink() {
            sink(SyncEvent::Invites(lists.invites().into()));
        }
        true
    }

    fn spawn_invite_changes(&self) {
        let demo = invites::scenario();
        if demo.arrives_late {
            self.spawn_invite_change(invites::LATE_ARRIVAL, |overrides| {
                overrides.held_back.remove(invites::LATE_INVITE);
            });
        }
        if demo.gets_revoked {
            self.spawn_invite_change(invites::REVOKED_AFTER, |overrides| {
                overrides
                    .withdrawn
                    .insert(invites::REVOKED_INVITE.to_owned());
            });
        }
    }

    fn spawn_invite_change(
        &self,
        after: Duration,
        change: impl FnOnce(&mut data::RoomOverrides) + Send + 'static,
    ) {
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(after).await;
            if let Ok(mut lists) = lists.lock() {
                change(&mut lists.overrides);
            }
            tracing::debug!("demo: the invites changed");
            sink(SyncEvent::Invites(current_lists(&lists).invites().into()));
        });
    }

    pub(super) fn echo_rooms(&self) {
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(room_info::ECHO_LAG).await;
            sink(SyncEvent::Rooms(current_lists(&lists).rooms().into()));
        });
    }

    fn address_taken(&self, wanted: &str) -> bool {
        let created = current_lists(&self.lists).overrides.created;
        data::alias_localparts().any(|taken| taken == wanted)
            || created.iter().any(|created| {
                created.alias.as_deref().is_some_and(|alias| {
                    alias
                        .strip_prefix('#')
                        .and_then(|alias| alias.split_once(':'))
                        .is_some_and(|(taken, _)| taken == wanted)
                })
            })
    }

    fn list_child(&self, parent: &RoomId, child: &RoomId) {
        self.change_lists(|lists| {
            lists
                .overrides
                .created_children
                .entry(parent.to_string())
                .or_default()
                .push(child.to_string());
        });
    }

    fn list_child_later(&self, parent: RoomId, child: RoomId) {
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(create_room::PARENT_LAG).await;
            if let Ok(mut lists) = lists.lock() {
                lists
                    .overrides
                    .created_children
                    .entry(parent.to_string())
                    .or_default()
                    .push(child.to_string());
            }
            tracing::debug!(%parent, %child, "demo: the parent space lists its new child");
            sink(SyncEvent::Spaces(current_lists(&lists).spaces().into()));
        });
    }
}

#[async_trait]
impl RoomCreationPort for DemoAuthed {
    async fn create_room(&self, blueprint: &RoomBlueprint) -> CreateResult {
        create_room::pause().await;
        let scenario = create_room::scenario();
        if scenario.refused {
            return Err(CreateFailure::Refused);
        }
        if scenario.fails {
            return Err(CreateFailure::Failed);
        }
        let own_server = server_part(data::own_user());
        let alias = match blueprint.access() {
            JoinAccess::Public(address) if self.address_taken(address.as_str()) => {
                return Err(CreateFailure::AddressTaken);
            }
            JoinAccess::Public(address) => Some(address.alias_on(own_server)),
            JoinAccess::InviteOnly | JoinAccess::SpaceMembers(_) => None,
        };
        let serial = self
            .creations
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let room_id = RoomId::new(data::created_room_id(serial));
        let child = match blueprint.parent() {
            None => ChildLink::NotAsked,
            Some(_) if scenario.child_fails => ChildLink::Failed,
            Some(_) => ChildLink::Added,
        };
        tracing::info!(%room_id, kind = ?blueprint.kind(), ?child, "demo: creating a room");
        if scenario.echo_lost {
            return Ok(CreatedRoom { room_id, child });
        }
        let created = data::Created {
            id: room_id.to_string(),
            space: blueprint.kind() == RoomKind::Space,
            name: blueprint.name().to_owned(),
            topic: blueprint.topic().map(str::to_owned),
            alias,
            encrypted: matches!(blueprint.shape(), Shape::Room { encrypted: true }),
            at_ms: unix_now_ms(),
        };
        self.change_lists(|lists| lists.overrides.created.push(created));
        if let (ChildLink::Added, Some(parent)) = (child, blueprint.parent()) {
            if scenario.parent_late {
                self.list_child_later(parent.clone(), room_id.clone());
            } else {
                self.list_child(parent, &room_id);
            }
        }
        self.echo_join();
        Ok(CreatedRoom { room_id, child })
    }
}

#[async_trait]
impl SpaceIndexPort for DemoAuthed {
    async fn hierarchy_page(&self, space_id: &RoomId, from: Option<&str>) -> Result<HierarchyPage> {
        space_index::pause().await;
        let offset = from.and_then(|token| token.parse().ok()).unwrap_or(0);
        if space_index::page_fails_now(space_id, offset) {
            return Err(unavailable("this page of the space index"));
        }
        let children = data::space_children(space_id);
        let next = offset.saturating_add(space_index::PAGE_SIZE);
        Ok(HierarchyPage {
            next: (next < children.len()).then(|| next.to_string()),
            children: children
                .into_iter()
                .skip(offset)
                .take(space_index::PAGE_SIZE)
                .collect(),
        })
    }

    async fn join(&self, room_id: &RoomId, via: &[String]) -> Result<()> {
        space_index::pause().await;
        if space_index::scenario().join_fails {
            return Err(unavailable("joining rooms"));
        }
        tracing::debug!(%room_id, ?via, "demo: joining a room from the space index");
        self.remember_join(room_id);
        self.echo_join();
        Ok(())
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        space_index::pause_avatars().await;
        media::fetch_unjoined_avatars(mxcs)
    }
}

#[async_trait]
impl InvitePort for DemoAuthed {
    async fn accept(&self, room_id: &RoomId) -> Result<()> {
        invites::pause().await;
        if invites::scenario().accept_fails {
            return Err(unavailable("accepting invites"));
        }
        tracing::debug!(%room_id, "demo: accepting an invite");
        if invites::scenario().echo_lost {
            return Ok(());
        }
        self.remember_join(room_id);
        self.echo_join();
        Ok(())
    }

    async fn decline(&self, room_id: &RoomId) -> Result<()> {
        invites::pause().await;
        if invites::scenario().decline_fails {
            return Err(unavailable("declining invites"));
        }
        tracing::debug!(%room_id, "demo: declining an invite");
        self.change_lists(|lists| {
            lists.overrides.declined.insert(room_id.to_string());
        });
        self.echo_join();
        Ok(())
    }
}

#[async_trait]
impl UpgradePort for DemoAuthed {
    async fn join_successor(&self, room_id: &RoomId) -> Result<()> {
        self.join_linked(room_id, data::successor_of(room_id)).await
    }

    async fn join_predecessor(&self, room_id: &RoomId) -> Result<()> {
        self.join_linked(room_id, data::predecessor_of(room_id))
            .await
    }
}

impl DemoAuthed {
    async fn join_linked(&self, room_id: &RoomId, linked: Option<RoomId>) -> Result<()> {
        upgrades::pause().await;
        if upgrades::scenario().join_fails {
            return Err(unavailable("joining across a room upgrade"));
        }
        let Some(linked) = linked else {
            return Err(AppError::Other(format!("no upgrade links {room_id}")));
        };
        tracing::debug!(%room_id, %linked, "demo: joining the other end of a room upgrade");
        if upgrades::scenario().echo_lost {
            return Ok(());
        }
        self.remember_join(&linked);
        self.echo_join();
        Ok(())
    }
}

#[async_trait]
impl RoomPreviewPort for DemoAuthed {
    async fn preview(&self, room: &RoomAddress, via: &[String]) -> PreviewResult {
        links::pause().await;
        tracing::debug!(%room, ?via, "demo: previewing a linked room");
        if links::preview_fails_now(room.as_str()) {
            return Err(PreviewFailure::Failed);
        }
        let room_id = match room {
            RoomAddress::Id(room_id) => room_id.clone(),
            RoomAddress::Alias(alias) => {
                data::alias_target(alias).ok_or(PreviewFailure::NoSuchRoom)?
            }
        };
        let membership = self.membership_of(&room_id);
        if links::scenario().unavailable && membership == PreviewMembership::Outsider {
            return Err(PreviewFailure::Unavailable);
        }
        let mut preview = data::room_card(&room_id).ok_or(PreviewFailure::Unavailable)?;
        preview.membership = membership;
        Ok(preview)
    }

    async fn join(
        &self,
        room: &RoomAddress,
        known: Option<&RoomId>,
        via: &[String],
    ) -> Result<RoomId> {
        links::pause().await;
        if links::scenario().join_fails {
            return Err(unavailable("joining linked rooms"));
        }
        let room_id = match room {
            RoomAddress::Id(room_id) => Some(room_id.clone()),
            RoomAddress::Alias(alias) => data::alias_target(alias),
        }
        .or_else(|| known.cloned())
        .filter(|room_id| data::room_card(room_id).is_some())
        .ok_or_else(|| AppError::Other(format!("no demo room has the address {room}")))?;
        tracing::debug!(%room_id, ?via, "demo: joining a linked room");
        if !links::scenario().echo_lost {
            self.remember_join(&room_id);
            self.echo_join();
        }
        Ok(room_id)
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        links::pause().await;
        media::fetch_unjoined_avatars(mxcs)
    }
}

impl DemoAuthed {
    fn membership_of(&self, room_id: &RoomId) -> PreviewMembership {
        let lists = current_lists(&self.lists);
        let joined = lists.rooms().iter().any(|room| room.id == *room_id)
            || lists
                .spaces()
                .iter()
                .any(|space| space.id == room_id.as_ref());
        if joined {
            return PreviewMembership::Joined;
        }
        if lists.invites().iter().any(|invite| invite.id == *room_id) {
            return PreviewMembership::Invited;
        }
        PreviewMembership::Outsider
    }
}

#[async_trait]
impl PushPort for DemoAuthed {
    async fn follow_notifications(&self, notices_tx: mpsc::Sender<PushNotice>) -> Result<()> {
        for timed in notifications::script() {
            sleep(timed.after).await;
            let following = match timed.step {
                notifications::Step::Arrive(arrival) => self.arrive(&arrival, &notices_tx).await,
                notifications::Step::ReadElsewhere(room_id) => {
                    self.read_elsewhere(&room_id);
                    true
                }
                notifications::Step::Invite => self.release_notified_invite(&notices_tx).await,
            };
            if !following {
                break;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl LeavePort for DemoAuthed {
    async fn leave(&self, room_id: &RoomId) -> Result<()> {
        room_info::pause().await;
        departures::pause().await;
        if room_info::scenario().leave_fails {
            return Err(unavailable("leaving rooms"));
        }
        if departures::scenario().room_stays && room_id.as_ref() == departures::STAYING_ROOM {
            return Err(unavailable("leaving this room"));
        }
        if account::scenario().forgets_rooms {
            tracing::info!(%room_id, "demo: leaving a room; the homeserver forgets it by itself");
        } else {
            tracing::info!(%room_id, "demo: leaving and forgetting a room");
        }
        self.change_lists(|lists| {
            lists.joined.retain(|joined| joined != room_id);
            lists.overrides.left.insert(room_id.to_string());
        });
        self.echo_departure();
        Ok(())
    }

    async fn forget(&self, room_id: &RoomId) -> Result<()> {
        departures::pause().await;
        if departures::scenario().forget_fails {
            return Err(unavailable("forgetting rooms"));
        }
        tracing::info!(%room_id, "demo: forgetting the room you were removed from");
        self.change_lists(|lists| {
            lists.overrides.forgotten.insert(room_id.to_string());
        });
        self.echo_departure();
        Ok(())
    }

    async fn admin_standing(&self, room_id: &RoomId) -> Result<AdminStanding> {
        departures::pause().await;
        tracing::debug!(%room_id, "demo: reading who else manages the room");
        if departures::scenario().sole_admin {
            return Ok(AdminStanding::OnlyAdmin);
        }
        Ok(AdminStanding::NotOnlyAdmin)
    }
}

#[async_trait]
impl SpaceOrderPort for DemoAuthed {
    async fn set_space_order(&self, space_id: &RoomId, order: &str) -> Result<()> {
        tracing::debug!(%space_id, order, "demo: ignoring space order write");
        Ok(())
    }
}
