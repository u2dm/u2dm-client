use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::adapters::demo::matrix::{
    DemoAuthed, EVERY_LIST, ListKind, RoomLists, announce_lists, current_lists,
};
use crate::adapters::demo::{
    calls, create_room, data, departures, invites, notifications, room_info, space_index, sync,
    timeline,
};
use crate::domain::member_name::NamedUser;
use crate::domain::push::{NoticeContent, PushNotice};
use crate::domain::room::RoomId;
use crate::domain::sync::{SyncEvent, SyncOutcome, SyncSupport};
use crate::ports::matrix::{SyncPort, SyncSink};
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
        announce_lists(&on_sync, &self.lists, &EVERY_LIST);
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
                    let kinds = [ListKind::Removals, ListKind::Rooms, ListKind::Invites];
                    announce_lists(&on_sync, &self.lists, &kinds);
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
    pub(in crate::adapters::demo::matrix) fn sync_sink(&self) -> Option<SyncSink> {
        self.sync_sink.lock().ok().and_then(|sink| sink.clone())
    }

    pub(in crate::adapters::demo::matrix) fn change_lists(
        &self,
        change: impl FnOnce(&mut RoomLists),
    ) {
        if let Ok(mut lists) = self.lists.lock() {
            change(&mut lists);
        }
    }

    pub(in crate::adapters::demo::matrix) fn after_lag(
        &self,
        lag: Duration,
        then: impl FnOnce(&SyncSink, &Mutex<RoomLists>) + Send + 'static,
    ) {
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(lag).await;
            then(&sink, &lists);
        });
    }

    pub(super) fn remember_join(&self, room_id: &RoomId) {
        self.change_lists(|lists| {
            if !lists.joined.contains(room_id) {
                lists.joined.push(room_id.clone());
            }
            lists.overrides.left.remove(room_id.as_ref());
        });
    }

    pub(super) fn echo_join(&self) {
        self.after_lag(space_index::JOIN_ECHO_LAG, |sink, lists| {
            announce_lists(sink, lists, &EVERY_LIST);
        });
    }

    pub(super) fn echo_departure(&self) {
        self.after_lag(room_info::ECHO_LAG, |sink, lists| {
            announce_lists(sink, lists, &EVERY_LIST);
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
        self.after_lag(departures::KICK_AFTER, move |sink, lists| {
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
            announce_lists(sink, lists, &[ListKind::Removals, ListKind::Rooms]);
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

    pub(super) async fn arrive(
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
                sent_at: unix_now_ms(),
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

    pub(super) fn read_elsewhere(&self, room_id: &RoomId) {
        tracing::debug!(%room_id, "demo: a room is read on another device");
        self.change_lists(|lists| {
            lists.overrides.read.insert(room_id.to_string());
        });
        self.emit_rooms();
    }

    pub(super) async fn release_notified_invite(
        &self,
        notices_tx: &mpsc::Sender<PushNotice>,
    ) -> bool {
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
            sent_at: unix_now_ms(),
            content: NoticeContent::Invite {
                inviter: invite.inviter.clone(),
            },
            tweaks: notifications::SOUND_ONLY,
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
        self.after_lag(after, |sink, lists| {
            if let Ok(mut lists) = lists.lock() {
                change(&mut lists.overrides);
            }
            tracing::debug!("demo: the invites changed");
            announce_lists(sink, lists, &[ListKind::Invites]);
        });
    }

    pub(in crate::adapters::demo::matrix) fn echo_rooms(&self) {
        self.after_lag(room_info::ECHO_LAG, |sink, lists| {
            announce_lists(sink, lists, &[ListKind::Rooms]);
        });
    }

    pub(super) fn address_taken(&self, wanted: &str) -> bool {
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

    pub(super) fn list_child(&self, parent: &RoomId, child: &RoomId) {
        self.change_lists(|lists| {
            lists
                .overrides
                .created_children
                .entry(parent.to_string())
                .or_default()
                .push(child.to_string());
        });
    }

    pub(super) fn list_child_later(&self, parent: RoomId, child: RoomId) {
        self.after_lag(create_room::PARENT_LAG, move |sink, lists| {
            if let Ok(mut lists) = lists.lock() {
                lists
                    .overrides
                    .created_children
                    .entry(parent.to_string())
                    .or_default()
                    .push(child.to_string());
            }
            tracing::debug!(%parent, %child, "demo: the parent space lists its new child");
            announce_lists(sink, lists, &[ListKind::Spaces]);
        });
    }
}
