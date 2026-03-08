use std::sync::Arc;
use std::time::Duration;

use matrix_sdk::config::SyncSettings;
use matrix_sdk::ruma::api::client::error::ErrorKind as RumaErrorKind;
use matrix_sdk::{Client, LoopCtrl};

use crate::domain::models::{Room as DomainRoom, RoomId, SyncEvent};
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
    on_sync: Arc<dyn Fn(SyncEvent) + Send + Sync>,
) -> Result<()> {
    let settings = SyncSettings::default().timeout(Duration::from_secs(30));
    let sync_client = client.clone();

    client
        .sync_with_result_callback(settings, move |result| {
            let on_sync = Arc::clone(&on_sync);
            let client = sync_client.clone();
            async move {
                match result {
                    Ok(_) => {
                        let rooms = build_room_list(&client).await;
                        on_sync(SyncEvent::Rooms(rooms));
                        Ok(LoopCtrl::Continue)
                    }
                    Err(e) => {
                        if is_auth_error(&e) {
                            tracing::warn!("unrecoverable auth error in sync loop, stopping");
                            on_sync(SyncEvent::SessionExpired);
                            Ok(LoopCtrl::Break)
                        } else {
                            on_sync(SyncEvent::ConnectionError(e.to_string()));
                            Ok(LoopCtrl::Continue)
                        }
                    }
                }
            }
        })
        .await?;

    Ok(())
}
