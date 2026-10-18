use matrix_sdk::{EncryptionState, Room};
use serde::Serialize;
use tokio::time::timeout;

use super::PRE_QUEUE_LOOKUP_WAIT;
use crate::adapters::draft_size::{self, MediaDraft, RoomFacts};
use crate::adapters::markdown::RoomMentions;
use crate::adapters::matrix::permissions::message_permissions;
use crate::adapters::matrix::room_state::event_ids;
use crate::domain::event_size::{Encryption, EventSize};
use crate::error::Result;

pub(in crate::adapters::matrix) async fn refuse_oversized(
    room: &Room,
    content: &(impl Serialize + Sync),
    event_type: &str,
) -> Result<()> {
    let content = serde_json::to_value(content)?;
    refuse_unless_it_fits(room, |facts| {
        draft_size::event_size(&content, event_type, facts)
    })
    .await
}

pub(super) async fn refuse_oversized_media(room: &Room, draft: &MediaDraft<'_>) -> Result<()> {
    refuse_unless_it_fits(room, |facts| draft_size::media(draft, facts)).await
}

async fn refuse_unless_it_fits(
    room: &Room,
    measure: impl FnOnce(&RoomFacts<'_>) -> EventSize + Send,
) -> Result<()> {
    let encryption = encryption(room).await;
    let room_mentions = RoomMentions::when(message_permissions(room).await.notify_room);
    let client = room.client();
    let facts = RoomFacts {
        room_id: room.room_id().as_str(),
        sender: room.own_user_id().as_str(),
        device_id: client.device_id().map_or("", |device| device.as_str()),
        encryption,
        event_ids: event_ids(room.version().as_ref()),
        room_mentions,
    };
    if let Err(oversize) = measure(&facts).checked() {
        tracing::info!(room_id = %room.room_id(), %oversize, "refusing an event over the size limit");
        return Err(oversize.into());
    }
    Ok(())
}

async fn encryption(room: &Room) -> Encryption {
    match timeout(PRE_QUEUE_LOOKUP_WAIT, room.latest_encryption_state()).await {
        Ok(Ok(EncryptionState::NotEncrypted)) => Encryption::Plain,
        Ok(Ok(_)) => Encryption::Megolm,
        Ok(Err(e)) => {
            tracing::warn!(
                room_id = %room.room_id(),
                "measuring the event as encrypted, since the room's encryption failed to load: {e}"
            );
            Encryption::Megolm
        }
        Err(_) => {
            tracing::warn!(
                room_id = %room.room_id(),
                waited = ?PRE_QUEUE_LOOKUP_WAIT,
                "measuring the event as encrypted, since the room's encryption did not arrive in time"
            );
            Encryption::Megolm
        }
    }
}
