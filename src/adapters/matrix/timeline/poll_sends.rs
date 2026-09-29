use std::future::pending;
use std::sync::Arc;

use matrix_sdk::Room;
use matrix_sdk::ruma::OwnedEventId;
use matrix_sdk::ruma::events::poll::unstable_end::UnstablePollEndEventContent;
use matrix_sdk::ruma::events::poll::unstable_response::UnstablePollResponseEventContent;
use matrix_sdk::ruma::events::poll::unstable_start::UnstablePollStartEventContent;
use matrix_sdk::ruma::events::{AnyMessageLikeEventContent, StaticEventContent};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::send_queue::{LocalEcho, LocalEchoContent, RoomSendQueueUpdate, SendHandle};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use super::poll_ends::{EndingPolls, send_end};
use crate::domain::poll::PollAction;
use crate::domain::timeline::TimelineUpdate;

pub(super) enum PollSendEvent {
    Queue(Result<RoomSendQueueUpdate, RecvError>),
    EndNotSent(OwnedEventId),
}

pub(super) struct PollSendGuard {
    room: Room,
    updates: Option<Receiver<RoomSendQueueUpdate>>,
    ending: Arc<EndingPolls>,
    unsent_end_tx: mpsc::UnboundedSender<OwnedEventId>,
    unsent_end_rx: mpsc::UnboundedReceiver<OwnedEventId>,
}

impl PollSendGuard {
    pub(super) async fn watch(
        room: Room,
        ending: Arc<EndingPolls>,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) -> Self {
        let updates = match room.send_queue().subscribe().await {
            Ok((echoes, updates)) => {
                reap(&room, &echoes, timeline_tx).await;
                Some(updates)
            }
            Err(e) => {
                tracing::warn!(room_id = %room.room_id(), "cannot watch this room's poll sends: {e}");
                None
            }
        };
        let (unsent_end_tx, unsent_end_rx) = mpsc::unbounded_channel();
        Self {
            room,
            updates,
            ending,
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
            self.unsent_end_tx.clone(),
        );
    }

    pub(super) async fn next(&mut self) -> PollSendEvent {
        let Self {
            updates,
            unsent_end_rx,
            ..
        } = self;
        tokio::select! {
            update = queue_update(updates.as_mut()) => PollSendEvent::Queue(update),
            Some(poll) = unsent_end_rx.recv() => PollSendEvent::EndNotSent(poll),
        }
    }

    pub(super) async fn settle(
        &mut self,
        event: PollSendEvent,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) -> Option<OwnedEventId> {
        match event {
            PollSendEvent::Queue(
                Ok(RoomSendQueueUpdate::SendError {
                    is_recoverable: false,
                    ..
                })
                | Err(RecvError::Lagged(_)),
            ) => self.rescan(timeline_tx).await,
            PollSendEvent::Queue(Err(RecvError::Closed)) => self.updates = None,
            PollSendEvent::Queue(Ok(_)) => {}
            PollSendEvent::EndNotSent(poll) => {
                self.ending.abandon(&poll);
                report(PollAction::End, timeline_tx).await;
                return Some(poll);
            }
        }
        None
    }

    async fn rescan(&self, timeline_tx: &mpsc::Sender<TimelineUpdate>) {
        match self.room.send_queue().subscribe().await {
            Ok((echoes, _updates)) => reap(&self.room, &echoes, timeline_tx).await,
            Err(e) => {
                tracing::warn!(room_id = %self.room.room_id(), "cannot rescan this room's poll sends: {e}");
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

async fn reap(room: &Room, echoes: &[LocalEcho], timeline_tx: &mpsc::Sender<TimelineUpdate>) {
    let mut reaped = false;
    for (action, handle) in echoes.iter().filter_map(wedged_poll_send) {
        match handle.abort().await {
            Ok(true) => {
                reaped = true;
                tracing::warn!(?action, room_id = %room.room_id(), "discarded a wedged poll send");
                report(action, timeline_tx).await;
            }
            Ok(false) => {}
            Err(e) => tracing::warn!(?action, "failed to discard a wedged poll send: {e}"),
        }
    }
    if reaped {
        room.send_queue().set_enabled(true);
    }
}

fn wedged_poll_send(echo: &LocalEcho) -> Option<(PollAction, &SendHandle)> {
    let LocalEchoContent::Event {
        serialized_event,
        send_handle,
        send_error: Some(_),
    } = &echo.content
    else {
        return None;
    };
    let (content, event_type) = serialized_event.raw();
    let action = if event_type == UnstablePollResponseEventContent::TYPE {
        PollAction::Vote
    } else if event_type == UnstablePollEndEventContent::TYPE {
        PollAction::End
    } else if event_type == UnstablePollStartEventContent::TYPE && replaces_a_poll(content) {
        PollAction::Edit
    } else {
        return None;
    };
    Some((action, send_handle))
}

fn replaces_a_poll(content: &Raw<AnyMessageLikeEventContent>) -> bool {
    matches!(
        content.deserialize_as_unchecked::<UnstablePollStartEventContent>(),
        Ok(UnstablePollStartEventContent::Replacement(_))
    )
}
