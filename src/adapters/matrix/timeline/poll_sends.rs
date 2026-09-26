use std::future::pending;

use matrix_sdk::Room;
use matrix_sdk::ruma::events::poll::unstable_end::UnstablePollEndEventContent;
use matrix_sdk::ruma::events::poll::unstable_response::UnstablePollResponseEventContent;
use matrix_sdk::ruma::events::poll::unstable_start::UnstablePollStartEventContent;
use matrix_sdk::ruma::events::{AnyMessageLikeEventContent, StaticEventContent};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::send_queue::{LocalEcho, LocalEchoContent, RoomSendQueueUpdate, SendHandle};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::domain::poll::PollAction;
use crate::domain::timeline::TimelineUpdate;

pub(super) struct PollSendGuard {
    room: Room,
    updates: Option<Receiver<RoomSendQueueUpdate>>,
}

impl PollSendGuard {
    pub(super) async fn watch(room: Room, timeline_tx: &mpsc::Sender<TimelineUpdate>) -> Self {
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
        Self { room, updates }
    }

    pub(super) fn room(&self) -> &Room {
        &self.room
    }

    pub(super) async fn next(&mut self) -> Result<RoomSendQueueUpdate, RecvError> {
        match self.updates.as_mut() {
            Some(updates) => updates.recv().await,
            None => pending().await,
        }
    }

    pub(super) async fn settle(
        &mut self,
        update: Result<RoomSendQueueUpdate, RecvError>,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) {
        match update {
            Ok(RoomSendQueueUpdate::SendError {
                is_recoverable: false,
                ..
            })
            | Err(RecvError::Lagged(_)) => self.rescan(timeline_tx).await,
            Err(RecvError::Closed) => self.updates = None,
            Ok(_) => {}
        }
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

async fn reap(room: &Room, echoes: &[LocalEcho], timeline_tx: &mpsc::Sender<TimelineUpdate>) {
    let mut reaped = false;
    for (action, handle) in echoes.iter().filter_map(wedged_poll_send) {
        match handle.abort().await {
            Ok(true) => {
                reaped = true;
                tracing::warn!(?action, room_id = %room.room_id(), "discarded a wedged poll send");
                drop(
                    timeline_tx
                        .send(TimelineUpdate::PollSendFailed(action))
                        .await,
                );
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
