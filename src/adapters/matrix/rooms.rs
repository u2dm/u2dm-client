use futures_util::StreamExt;
use matrix_sdk::Client;
use matrix_sdk::config::SyncSettings;
use matrix_sdk::ruma::api::client::error::ErrorKind as RumaErrorKind;
use matrix_sdk::ruma::events::AnyMessageLikeEventContent;
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
use matrix_sdk::ruma::{IdParseError, OwnedRoomId};
use tokio::sync::mpsc;

use crate::domain::models::{
    ConnectionStatus, Room as DomainRoom, RoomId, SyncEvent, SyncSnapshot,
};
use crate::error::{AppError, Result};

pub(super) async fn build_room_list(client: &Client) -> Vec<DomainRoom> {
    let mut rooms = Vec::new();
    for room in client.joined_rooms() {
        let display_name = room
            .display_name()
            .await
            .map(|dn| dn.to_string())
            .unwrap_or_default();
        let unread = room.unread_notification_counts().notification_count;
        let is_direct = room.is_direct().await.unwrap_or_default();
        let last_activity_ts: u64 = room
            .new_latest_event_timestamp()
            .map_or(0, |ts| ts.0.into());
        rooms.push(DomainRoom {
            id: RoomId(room.room_id().to_string()),
            display_name,
            is_direct,
            unread_count: unread,
            last_activity_ts,
        });
    }
    rooms.sort_by(|a, b| {
        b.unread_count
            .min(1)
            .cmp(&a.unread_count.min(1))
            .then(b.last_activity_ts.cmp(&a.last_activity_ts))
    });
    rooms
}

pub(super) fn is_auth_error(err: &matrix_sdk::Error) -> bool {
    if matches!(
        err.client_api_error_kind(),
        Some(
            RumaErrorKind::Unauthorized
                | RumaErrorKind::Forbidden { .. }
                | RumaErrorKind::UnknownToken { .. }
        )
    ) {
        return true;
    }

    matches!(
        err,
        matrix_sdk::Error::Http(http_err) if matches!(http_err.as_ref(), matrix_sdk::HttpError::RefreshToken(_))
    )
}

pub(super) async fn fetch_rooms(client: &Client) -> Result<Vec<DomainRoom>> {
    client
        .event_cache()
        .subscribe()
        .map_err(|e| AppError::Other(e.to_string()))?;
    if let Err(e) = client.sync_once(SyncSettings::default()).await {
        if is_auth_error(&e) {
            return Err(AppError::SessionExpired);
        }
        return Err(e.into());
    }
    Ok(build_room_list(client).await)
}

pub(super) async fn start_sync(
    client: &Client,
    state_tx: mpsc::UnboundedSender<SyncEvent>,
) -> Result<()> {
    let stream = client.sync_stream(SyncSettings::default()).await;
    tokio::pin!(stream);

    while let Some(result) = stream.next().await {
        let event = match result {
            Ok(_) => SyncEvent::Snapshot(SyncSnapshot {
                rooms: build_room_list(client).await,
                connection_status: ConnectionStatus::Connected,
            }),
            Err(e) => {
                if is_auth_error(&e) {
                    tracing::warn!("unrecoverable auth error in sync loop, stopping");
                    state_tx.send(SyncEvent::SessionExpired).ok();
                    return Ok(());
                }
                SyncEvent::Snapshot(SyncSnapshot {
                    rooms: Vec::new(),
                    connection_status: ConnectionStatus::Error(e.to_string()),
                })
            }
        };
        if state_tx.send(event).is_err() {
            break;
        }
    }

    state_tx.send(SyncEvent::Ended).ok();
    Ok(())
}

pub(super) async fn send_text(client: &Client, room_id: &RoomId, body: &str) -> Result<()> {
    let room_id_parsed: OwnedRoomId = room_id
        .0
        .as_str()
        .try_into()
        .map_err(|e: IdParseError| AppError::Other(e.to_string()))?;

    let room = client
        .get_room(&room_id_parsed)
        .ok_or_else(|| AppError::Other("Room not found".into()))?;

    let content: AnyMessageLikeEventContent = RoomMessageEventContent::text_plain(body).into();
    room.send_queue()
        .send(content)
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;

    Ok(())
}
