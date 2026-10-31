use matrix_sdk::Room;
use matrix_sdk::config::RequestConfig;
use matrix_sdk::ruma::{EventId, OwnedEventId, OwnedUserId};
use matrix_sdk::send_queue::LocalEchoContent;

use super::PRE_QUEUE_LOOKUP_WAIT;
use crate::adapters::matrix::permissions::message_permissions;
use crate::adapters::matrix::requests::in_time;
use crate::error::{AppError, Result};

pub(super) async fn delete(room: &Room, event_id: &str) -> Result<()> {
    let event_id = OwnedEventId::try_from(event_id).map_err(AppError::other)?;
    if already_queued(room, &event_id).await {
        tracing::debug!(%event_id, "the deletion is already queued");
        return Ok(());
    }
    let sender = sender_of(room, &event_id).await?;
    let permissions = message_permissions(room).await;
    let allowed = if sender == room.own_user_id() {
        permissions.delete_own
    } else {
        permissions.delete_others
    };
    if !allowed {
        return Err(AppError::Other(format!(
            "the power levels do not let you delete {event_id}"
        )));
    }
    room.send_queue()
        .redact(event_id, None)
        .await
        .map(drop)
        .map_err(AppError::other)
}

async fn already_queued(room: &Room, event_id: &EventId) -> bool {
    let Ok((echoes, _updates)) = room.send_queue().subscribe().await else {
        return false;
    };
    echoes.iter().any(|echo| {
        matches!(
            &echo.content,
            LocalEchoContent::Redaction { redacts, .. } if redacts == event_id
        )
    })
}

async fn sender_of(room: &Room, event_id: &EventId) -> Result<OwnedUserId> {
    let event = in_time(
        PRE_QUEUE_LOOKUP_WAIT,
        format!("the event {event_id} to delete did not arrive in time"),
        room.load_or_fetch_event(event_id, Some(RequestConfig::short_retry())),
    )
    .await?;
    event
        .raw()
        .get_field::<OwnedUserId>("sender")
        .ok()
        .flatten()
        .ok_or_else(|| AppError::Other(format!("{event_id} names no sender")))
}
