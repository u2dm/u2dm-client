use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use matrix_sdk::Room;
use matrix_sdk::ruma::api::client::state::get_state_event_for_key::v3::{
    Request as StateRequest, Response as StateResponse, StateEventFormat,
};
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::events::{AnySyncStateEvent, StateEventType};
use matrix_sdk_base::{RawStateEventWithKeys, RoomInfoNotableUpdateReasons};
use matrix_sdk_ui::eyeball_im::{Vector, VectorDiff};
use matrix_sdk_ui::timeline::{
    RoomExt as _, TimelineFocus as SdkTimelineFocus, TimelineItem, TimelineItemContent,
};
use tokio::sync::mpsc;

use super::convert::content_preview;
use crate::adapters::matrix::session::ClientHandle;
use crate::domain::message::{MessagePreviewKind, PinnedMessage};
use crate::domain::room::RoomId;
use crate::error::{AppError, Result};
use crate::ports::matrix::PinnedPort;

type PinnedIds = RawStateEventWithKeys<AnySyncStateEvent>;

const REDACTED_PIN: &str = "redacted";

pub(in crate::adapters::matrix) struct MatrixPinned {
    matrix: Arc<ClientHandle>,
}

impl MatrixPinned {
    pub(in crate::adapters::matrix) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self { matrix }
    }
}

#[async_trait]
impl PinnedPort for MatrixPinned {
    async fn subscribe_pinned(
        &self,
        room_id: &RoomId,
        pinned_tx: mpsc::Sender<Vec<PinnedMessage>>,
    ) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        let timeline = room
            .timeline_builder()
            .with_focus(SdkTimelineFocus::PinnedEvents)
            .build()
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        let (items, diffs) = timeline.subscribe().await;
        tokio::join!(
            refresh_pinned_ids(&room),
            follow_pinned_items(room_id, items, diffs, &pinned_tx),
        );
        Ok(())
    }
}

async fn follow_pinned_items<S>(
    room_id: &RoomId,
    mut items: Vector<Arc<TimelineItem>>,
    mut diffs: S,
    pinned_tx: &mpsc::Sender<Vec<PinnedMessage>>,
) where
    S: Stream<Item = Vec<VectorDiff<Arc<TimelineItem>>>> + Unpin,
{
    let mut sent = pinned_messages(&items);
    tracing::debug!(
        %room_id,
        pinned = sent.len(),
        skipped = ?skipped_pins(&items),
        "loaded the pinned messages"
    );
    if pinned_tx.send(sent.clone()).await.is_err() {
        return;
    }
    while let Some(batch) = diffs.next().await {
        for diff in batch {
            diff.apply(&mut items);
        }
        let pinned = pinned_messages(&items);
        tracing::debug!(
            %room_id,
            pinned = pinned.len(),
            skipped = ?skipped_pins(&items),
            "the pinned timeline changed"
        );
        if pinned == sent {
            continue;
        }
        if pinned_tx.send(pinned.clone()).await.is_err() {
            return;
        }
        sent = pinned;
    }
}

async fn refresh_pinned_ids(room: &Room) {
    let known = room.pinned_event_ids();
    let Some(mut fetched) = fetch_pinned_ids(room).await else {
        return;
    };
    let mut probe = room.clone_info();
    probe.handle_state_event(&mut fetched);
    let pinned = probe.pinned_event_ids();
    let changed = pinned != known;
    tracing::debug!(
        room_id = %room.room_id(),
        known = ?known.as_ref().map(Vec::len),
        pinned = ?pinned.as_ref().map(Vec::len),
        changed,
        "fetched the room's pinned event ids"
    );
    if !changed {
        return;
    }
    let saved = room
        .update_and_save_room_info(|mut info| {
            if info.pinned_event_ids() == known {
                info.handle_state_event(&mut fetched);
            }
            (info, RoomInfoNotableUpdateReasons::empty())
        })
        .await;
    if let Err(e) = saved {
        tracing::warn!(room_id = %room.room_id(), "failed to save the room's pinned event ids: {e}");
    }
}

async fn fetch_pinned_ids(room: &Room) -> Option<PinnedIds> {
    let mut request = StateRequest::new(
        room.room_id().to_owned(),
        StateEventType::RoomPinnedEvents,
        String::new(),
    );
    request.format = StateEventFormat::Event;
    match room.client().send(request).await {
        Ok(response) => parse_pinned_ids(room, response),
        Err(e) if e.client_api_error_kind() == Some(&ErrorKind::NotFound) => {
            tracing::debug!(room_id = %room.room_id(), "the room has never pinned a message");
            None
        }
        Err(e) => {
            tracing::warn!(room_id = %room.room_id(), "failed to fetch the room's pinned event ids: {e}");
            None
        }
    }
}

fn parse_pinned_ids(room: &Room, response: StateResponse) -> Option<PinnedIds> {
    let fetched = RawStateEventWithKeys::try_from_raw_state_event(response.into_event().cast());
    if fetched.is_none() {
        tracing::warn!(room_id = %room.room_id(), "the server sent the pinned event ids without their event");
    }
    fetched
}

fn skipped_pins(items: &Vector<Arc<TimelineItem>>) -> Vec<String> {
    items
        .iter()
        .filter(|item| pinned_message(item).is_none())
        .filter_map(|item| item.as_event())
        .map(|event| unpreviewable_kind(event.content()))
        .collect()
}

fn unpreviewable_kind(content: &TimelineItemContent) -> String {
    content
        .event_type_str()
        .unwrap_or_else(|| REDACTED_PIN.to_owned())
}

fn pinned_messages(items: &Vector<Arc<TimelineItem>>) -> Vec<PinnedMessage> {
    items
        .iter()
        .filter_map(|item| pinned_message(item))
        .collect()
}

fn pinned_message(item: &TimelineItem) -> Option<PinnedMessage> {
    let event = item.as_event()?;
    let (kind, body) = content_preview(event.content());
    if kind == MessagePreviewKind::None {
        return None;
    }
    Some(PinnedMessage {
        event_id: event.event_id()?.to_string(),
        kind,
        body,
    })
}
