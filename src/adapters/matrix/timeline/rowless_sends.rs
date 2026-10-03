use std::future::pending;
use std::sync::Arc;

use matrix_sdk::Room;
use matrix_sdk::ruma::OwnedEventId;
use matrix_sdk::ruma::events::poll::unstable_end::UnstablePollEndEventContent;
use matrix_sdk::ruma::events::poll::unstable_response::UnstablePollResponseEventContent;
use matrix_sdk::ruma::events::poll::unstable_start::UnstablePollStartEventContent;
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
use matrix_sdk::ruma::events::{AnyMessageLikeEventContent, StaticEventContent};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::send_queue::{LocalEcho, LocalEchoContent, RoomSendQueueUpdate, SendHandle};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio_util::task::TaskTracker;

use super::edits::{DiscardedEdits, replaced_text};
use super::poll_ends::{EndingPolls, send_end};
use crate::domain::message::MessageEdit;
use crate::domain::poll::PollAction;
use crate::domain::timeline::TimelineUpdate;

pub(super) enum RowlessSendEvent {
    Queue(Result<RoomSendQueueUpdate, RecvError>),
    EndNotSent(OwnedEventId),
}

#[derive(Debug)]
enum Rowless {
    Poll(PollAction),
    Edit(MessageEdit),
}

pub(super) struct RowlessSendGuard {
    room: Room,
    updates: Option<Receiver<RoomSendQueueUpdate>>,
    ending: Arc<EndingPolls>,
    discarded: Arc<DiscardedEdits>,
    ends: TaskTracker,
    unsent_end_tx: mpsc::UnboundedSender<OwnedEventId>,
    unsent_end_rx: mpsc::UnboundedReceiver<OwnedEventId>,
}

impl RowlessSendGuard {
    pub(super) async fn watch(
        room: Room,
        ending: Arc<EndingPolls>,
        discarded: Arc<DiscardedEdits>,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) -> Self {
        let updates = match room.send_queue().subscribe().await {
            Ok((echoes, updates)) => {
                reap(&room, &echoes, &discarded, timeline_tx).await;
                Some(updates)
            }
            Err(e) => {
                tracing::warn!(room_id = %room.room_id(), "cannot watch this room's rowless sends: {e}");
                None
            }
        };
        let (unsent_end_tx, unsent_end_rx) = mpsc::unbounded_channel();
        Self {
            room,
            updates,
            ending,
            discarded,
            ends: TaskTracker::new(),
            unsent_end_tx,
            unsent_end_rx,
        }
    }

    pub(super) fn room(&self) -> &Room {
        &self.room
    }

    pub(super) fn end(&self, content: UnstablePollEndEventContent) {
        send_end(
            self.room.clone(),
            &self.ending,
            content,
            &self.ends,
            self.unsent_end_tx.clone(),
        );
    }

    pub(super) async fn report_unsent_ends(&mut self, timeline_tx: &mpsc::Sender<TimelineUpdate>) {
        self.ends.close();
        self.ends.wait().await;
        while self.unsent_end_rx.try_recv().is_ok() {
            report(PollAction::End, timeline_tx).await;
        }
    }

    pub(super) async fn next(&mut self) -> RowlessSendEvent {
        let Self {
            updates,
            unsent_end_rx,
            ..
        } = self;
        tokio::select! {
            update = queue_update(updates.as_mut()) => RowlessSendEvent::Queue(update),
            Some(poll) = unsent_end_rx.recv() => RowlessSendEvent::EndNotSent(poll),
        }
    }

    pub(super) async fn settle(
        &mut self,
        event: RowlessSendEvent,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) -> Option<OwnedEventId> {
        match event {
            RowlessSendEvent::Queue(
                Ok(RoomSendQueueUpdate::SendError {
                    is_recoverable: false,
                    ..
                })
                | Err(RecvError::Lagged(_)),
            ) => self.rescan(timeline_tx).await,
            RowlessSendEvent::Queue(Err(RecvError::Closed)) => self.updates = None,
            RowlessSendEvent::Queue(Ok(_)) => {}
            RowlessSendEvent::EndNotSent(poll) => {
                self.ending.abandon(&poll);
                report(PollAction::End, timeline_tx).await;
                return Some(poll);
            }
        }
        None
    }

    async fn rescan(&self, timeline_tx: &mpsc::Sender<TimelineUpdate>) {
        match self.room.send_queue().subscribe().await {
            Ok((echoes, _updates)) => {
                reap(&self.room, &echoes, &self.discarded, timeline_tx).await;
            }
            Err(e) => {
                tracing::warn!(room_id = %self.room.room_id(), "cannot rescan this room's rowless sends: {e}");
            }
        }
    }
}

async fn queue_update(
    updates: Option<&mut Receiver<RoomSendQueueUpdate>>,
) -> Result<RoomSendQueueUpdate, RecvError> {
    match updates {
        Some(updates) => updates.recv().await,
        None => pending().await,
    }
}

pub(super) async fn report(action: PollAction, timeline_tx: &mpsc::Sender<TimelineUpdate>) {
    drop(
        timeline_tx
            .send(TimelineUpdate::PollSendFailed(action))
            .await,
    );
}

async fn reap(
    room: &Room,
    echoes: &[LocalEcho],
    discarded: &DiscardedEdits,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
) {
    let mut reaped = false;
    for (rowless, handle) in echoes.iter().filter_map(wedged_rowless_send) {
        match handle.abort().await {
            Ok(true) => {
                reaped = true;
                report_reaped(room, rowless, discarded, timeline_tx).await;
            }
            Ok(false) => {}
            Err(e) => tracing::warn!(?rowless, "failed to discard a wedged send: {e}"),
        }
    }
    if reaped {
        room.send_queue().set_enabled(true);
    }
}

async fn report_reaped(
    room: &Room,
    rowless: Rowless,
    discarded: &DiscardedEdits,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
) {
    match rowless {
        Rowless::Poll(action) => {
            tracing::warn!(?action, room_id = %room.room_id(), "discarded a wedged poll send");
            report(action, timeline_tx).await;
        }
        Rowless::Edit(edit) => {
            tracing::warn!(
                edited = ?edit.target,
                room_id = %room.room_id(),
                "discarded a wedged edit"
            );
            if let Some(target) = edit.target.event_id() {
                discarded.record(target);
            }
            drop(timeline_tx.send(TimelineUpdate::EditUnsaved(edit)).await);
        }
    }
}

fn wedged_rowless_send(echo: &LocalEcho) -> Option<(Rowless, &SendHandle)> {
    let LocalEchoContent::Event {
        serialized_event,
        send_handle,
        send_error: Some(_),
    } = &echo.content
    else {
        return None;
    };
    let (content, event_type) = serialized_event.raw();
    let rowless = if event_type == UnstablePollResponseEventContent::TYPE {
        Rowless::Poll(PollAction::Vote)
    } else if event_type == UnstablePollEndEventContent::TYPE {
        Rowless::Poll(PollAction::End)
    } else if event_type == UnstablePollStartEventContent::TYPE && replaces_a_poll(content) {
        Rowless::Poll(PollAction::Edit)
    } else if event_type == RoomMessageEventContent::TYPE {
        Rowless::Edit(replaced_text(content)?)
    } else {
        return None;
    };
    Some((rowless, send_handle))
}

fn replaces_a_poll(content: &Raw<AnyMessageLikeEventContent>) -> bool {
    matches!(
        content.deserialize_as_unchecked::<UnstablePollStartEventContent>(),
        Ok(UnstablePollStartEventContent::Replacement(_))
    )
}
