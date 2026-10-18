use matrix_sdk::Room;
use matrix_sdk::ruma::events::MessageLikeEventType;

use super::latest_encryption;
use crate::adapters::matrix::permissions::{may_send, message_permissions};
use crate::domain::message::ReactionSend;
use crate::error::{AppError, Result};

pub(in crate::adapters::matrix) async fn refuse_unpermitted(
    room: &Room,
    event_type: MessageLikeEventType,
) -> Result<()> {
    let encryption = latest_encryption(room).await;
    if may_send(room, event_type.clone(), encryption).await {
        return Ok(());
    }
    tracing::info!(
        room_id = %room.room_id(),
        %event_type,
        ?encryption,
        "refusing an event the power levels do not allow"
    );
    Err(AppError::SendNotAllowed)
}

pub(super) async fn refuse_unpermitted_toggle(
    room: &Room,
    own: Option<ReactionSend>,
) -> Result<()> {
    if message_permissions(room).await.allows_toggle(own) {
        return Ok(());
    }
    tracing::info!(
        room_id = %room.room_id(),
        ?own,
        "refusing a reaction toggle the power levels do not allow"
    );
    Err(AppError::SendNotAllowed)
}
