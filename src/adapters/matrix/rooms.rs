use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use matrix_sdk::config::SyncSettings;
use matrix_sdk::ruma::api::client::error::ErrorKind as RumaErrorKind;
use matrix_sdk::{Client, LoopCtrl, Room};
use tokio::sync::broadcast::error::RecvError;

use crate::domain::models::{Room as DomainRoom, RoomId, SyncEvent};
use crate::error::{AppError, Result};

async fn build_single_room(room: &Room) -> DomainRoom {
    let display_name = room
        .display_name()
        .await
        .map(|dn| dn.to_string())
        .unwrap_or_default();
    let unread = room.num_unread_notifications();
    let mentions = room.num_unread_mentions();
    let is_direct = room.is_direct().await.unwrap_or_default();
    let last_activity_ts: u64 = room
        .new_latest_event_timestamp()
        .map_or(0, |ts| ts.0.into());
    DomainRoom {
        id: RoomId(room.room_id().to_string()),
        display_name,
        is_direct,
        unread_count: unread,
        mention_count: mentions,
        last_activity_ts,
    }
}

fn sort_rooms(rooms: &mut [DomainRoom]) {
    rooms.sort_by(|a, b| {
        b.unread_count
            .min(1)
            .cmp(&a.unread_count.min(1))
            .then(b.last_activity_ts.cmp(&a.last_activity_ts))
    });
}

pub(super) async fn build_room_list(client: &Client) -> Vec<DomainRoom> {
    let mut rooms = Vec::new();
    for room in client.joined_rooms() {
        rooms.push(build_single_room(&room).await);
    }
    sort_rooms(&mut rooms);
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
    let mut room_updates_rx = client.subscribe_to_all_room_updates();
    let mut room_cache: HashMap<String, DomainRoom> = HashMap::new();
    for room in client.joined_rooms() {
        let dr = build_single_room(&room).await;
        room_cache.insert(dr.id.0.clone(), dr);
    }

    let sync_client = client.clone();
    let on_sync_errors = Arc::clone(&on_sync);
    let sync_task = tokio::spawn(async move {
        let settings = SyncSettings::default().timeout(Duration::from_secs(30));
        #[allow(clippy::let_underscore_must_use)]
        let _ = sync_client
            .sync_with_result_callback(settings, move |result| {
                let on_sync = Arc::clone(&on_sync_errors);
                async move {
                    match result {
                        Ok(_) => {
                            on_sync(SyncEvent::Connected);
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
            .await;
    });

    loop {
        match room_updates_rx.recv().await {
            Ok(updates) => {
                let has_joins = !updates.joined.is_empty();
                let has_leaves = !updates.left.is_empty();

                if !has_joins && !has_leaves {
                    continue;
                }

                for room_id in updates.left.keys() {
                    let key = room_id.to_string();
                    room_cache.remove(&key);
                }

                for room_id in updates.joined.keys() {
                    if let Some(room) = client.get_room(room_id) {
                        let dr = build_single_room(&room).await;
                        room_cache.insert(dr.id.0.clone(), dr);
                    }
                }

                let mut rooms: Vec<DomainRoom> = room_cache.values().cloned().collect();
                sort_rooms(&mut rooms);
                on_sync(SyncEvent::Rooms(rooms));
            }
            Err(RecvError::Lagged(n)) => {
                tracing::warn!("room updates lagged by {n} messages, full rebuild");
                room_cache.clear();
                for room in client.joined_rooms() {
                    let dr = build_single_room(&room).await;
                    room_cache.insert(dr.id.0.clone(), dr);
                }
                let mut rooms: Vec<DomainRoom> = room_cache.values().cloned().collect();
                sort_rooms(&mut rooms);
                on_sync(SyncEvent::Rooms(rooms));
            }
            Err(RecvError::Closed) => break,
        }
    }

    sync_task.abort();
    Ok(())
}
