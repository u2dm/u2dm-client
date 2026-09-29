use std::collections::HashSet;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use matrix_sdk::Room;
use matrix_sdk::config::RequestConfig;
use matrix_sdk::ruma::events::poll::unstable_end::UnstablePollEndEventContent;
use matrix_sdk::ruma::{EventId, OwnedEventId};
use matrix_sdk::send_queue::{LocalEcho, LocalEchoContent};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::task::TaskTracker;

use super::RELATION_FIELD;

const QUEUED_RELATIONS_WAIT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(in crate::adapters::matrix) struct EndingPolls {
    polls: StdMutex<HashSet<String>>,
}

impl EndingPolls {
    pub(super) fn contains(&self, poll: &str) -> bool {
        self.polls.lock().is_ok_and(|polls| polls.contains(poll))
    }

    fn begin(&self, poll: &EventId) {
        if let Ok(mut polls) = self.polls.lock() {
            polls.insert(poll.to_string());
        }
    }

    pub(super) fn abandon(&self, poll: &EventId) {
        if let Ok(mut polls) = self.polls.lock() {
            polls.remove(poll.as_str());
        }
    }
}

#[derive(Deserialize)]
struct Relation {
    event_id: OwnedEventId,
}

fn relates_to(echo: &LocalEcho, poll: &EventId) -> bool {
    let LocalEchoContent::Event {
        serialized_event, ..
    } = &echo.content
    else {
        return false;
    };
    let (content, _event_type) = serialized_event.raw();
    matches!(
        content.get_field::<Relation>(RELATION_FIELD),
        Ok(Some(relation)) if relation.event_id == poll
    )
}

async fn has_queued_relations(room: &Room, poll: &EventId) -> bool {
    match room.send_queue().subscribe().await {
        Ok((echoes, _updates)) => echoes.iter().any(|echo| relates_to(echo, poll)),
        Err(e) => {
            tracing::warn!(%poll, "cannot read the room's queued sends: {e}");
            false
        }
    }
}

async fn queued_relations_sent(room: &Room, poll: &EventId) -> bool {
    let Ok((echoes, mut updates)) = room.send_queue().subscribe().await else {
        return true;
    };
    if !echoes.iter().any(|echo| relates_to(echo, poll)) {
        return true;
    }
    let drained = async {
        loop {
            match updates.recv().await {
                Err(RecvError::Closed) => return,
                Ok(_) | Err(RecvError::Lagged(_)) => {
                    if !has_queued_relations(room, poll).await {
                        return;
                    }
                }
            }
        }
    };
    timeout(QUEUED_RELATIONS_WAIT, drained).await.is_ok()
}

async fn send_directly(room: &Room, poll: &EventId, content: UnstablePollEndEventContent) -> bool {
    match room
        .send(content)
        .with_request_config(RequestConfig::short_retry())
        .await
    {
        Ok(sent) => {
            tracing::info!(%poll, event_id = %sent.response.event_id, "ended a poll");
            true
        }
        Err(e) => {
            tracing::warn!(%poll, "the poll end was not sent: {e}");
            false
        }
    }
}

async fn end_after_queued_relations(
    room: &Room,
    poll: &EventId,
    content: UnstablePollEndEventContent,
) -> bool {
    if queued_relations_sent(room, poll).await {
        return send_directly(room, poll, content).await;
    }
    tracing::warn!(
        %poll,
        waited = ?QUEUED_RELATIONS_WAIT,
        "the poll end was not sent: your votes or edits on the poll are still queued"
    );
    false
}

pub(super) fn send_end(
    room: Room,
    ending: &EndingPolls,
    content: UnstablePollEndEventContent,
    ends: &TaskTracker,
    unsent_tx: mpsc::UnboundedSender<OwnedEventId>,
) {
    let poll = content.relates_to.event_id.clone();
    ending.begin(&poll);
    ends.spawn(async move {
        if !end_after_queued_relations(&room, &poll, content).await {
            drop(unsent_tx.send(poll));
        }
    });
}
