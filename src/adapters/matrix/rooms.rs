use std::collections::HashMap;
use std::sync::Arc;

use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::sync::RoomUpdates;
use matrix_sdk::{Client, HttpError, Room};
use matrix_sdk_ui::encryption_sync_service::Error as EncryptionSyncError;
use matrix_sdk_ui::room_list_service::Error as RoomListError;
use matrix_sdk_ui::sync_service::{Error as SyncServiceError, State as SyncState, SyncService};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

use crate::domain::models::{Room as DomainRoom, RoomId, SyncEvent};
use crate::error::{AppError, Result as AppResult};

async fn build_single_room(room: &Room) -> DomainRoom {
    let display_name = room
        .cached_display_name()
        .map(|dn| dn.to_string())
        .unwrap_or_default();
    let unread = room.num_unread_notifications();
    let mentions = room.num_unread_mentions();
    let is_direct = room.is_direct().await.unwrap_or_default();
    let last_activity_ts: u64 = room.latest_event_timestamp().map_or(0, |ts| ts.0.into());
    DomainRoom {
        id: RoomId::new(room.room_id().to_string()),
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

fn emit_room_update(
    room_cache: &HashMap<String, DomainRoom>,
    on_sync: &Arc<dyn Fn(SyncEvent) + Send + Sync>,
) {
    let mut rooms: Vec<DomainRoom> = room_cache.values().cloned().collect();
    sort_rooms(&mut rooms);
    on_sync(SyncEvent::Rooms(rooms));
}

async fn build_sync_service(client: &Client) -> AppResult<SyncService> {
    client
        .event_cache()
        .subscribe()
        .map_err(|e| AppError::Other(e.to_string()))?;

    SyncService::builder(client.clone())
        .build()
        .await
        .map_err(|e| AppError::Other(e.to_string()))
}

async fn seed_room_cache(client: &Client, room_cache: &mut HashMap<String, DomainRoom>) {
    for room in client.joined_rooms() {
        let dr = build_single_room(&room).await;
        room_cache.insert(dr.id.to_string(), dr);
    }
}

async fn apply_room_updates(
    client: &Client,
    updates: &RoomUpdates,
    room_cache: &mut HashMap<String, DomainRoom>,
) {
    for room_id in updates.left.keys() {
        let key = room_id.to_string();
        room_cache.remove(&key);
    }
    for room_id in updates.joined.keys() {
        if let Some(room) = client.get_room(room_id) {
            let dr = build_single_room(&room).await;
            room_cache.insert(dr.id.to_string(), dr);
        }
    }
}

fn extract_sdk_error(err: &SyncServiceError) -> Option<&matrix_sdk::Error> {
    match err {
        SyncServiceError::RoomList(RoomListError::SlidingSync(e))
        | SyncServiceError::EncryptionSync(EncryptionSyncError::SlidingSync(e)) => Some(e),
        _ => None,
    }
}

fn is_refresh_token_error(err: &matrix_sdk::Error) -> bool {
    matches!(
        err,
        matrix_sdk::Error::Http(http) if matches!(http.as_ref(), HttpError::RefreshToken(_))
    )
}

fn is_auth_error(err: &SyncServiceError) -> bool {
    extract_sdk_error(err).is_some_and(|e| {
        if matches!(
            e.client_api_error_kind(),
            Some(ErrorKind::UnknownToken { .. } | ErrorKind::Unauthorized | ErrorKind::Forbidden)
        ) {
            return true;
        }
        is_refresh_token_error(e)
    })
}

enum LoopAction {
    Continue,
    Break,
}

async fn handle_room_update(
    client: &Client,
    update: Result<RoomUpdates, RecvError>,
    room_cache: &mut HashMap<String, DomainRoom>,
    on_sync: &Arc<dyn Fn(SyncEvent) + Send + Sync>,
) -> LoopAction {
    match update {
        Ok(updates) => {
            if updates.joined.is_empty() && updates.left.is_empty() {
                return LoopAction::Continue;
            }
            tracing::debug!(
                joined = updates.joined.len(),
                left = updates.left.len(),
                "processing room updates"
            );
            apply_room_updates(client, &updates, room_cache).await;
            emit_room_update(room_cache, on_sync);
            LoopAction::Continue
        }
        Err(RecvError::Lagged(n)) => {
            tracing::warn!("room updates lagged by {n} messages, full rebuild");
            room_cache.clear();
            seed_room_cache(client, room_cache).await;
            emit_room_update(room_cache, on_sync);
            LoopAction::Continue
        }
        Err(RecvError::Closed) => LoopAction::Break,
    }
}

#[allow(clippy::cognitive_complexity)]
async fn handle_sync_state(
    client: &Client,
    state: SyncState,
    sync_service: &SyncService,
    room_cache: &mut HashMap<String, DomainRoom>,
    on_sync: &Arc<dyn Fn(SyncEvent) + Send + Sync>,
) -> LoopAction {
    match state {
        SyncState::Running => {
            tracing::info!("sliding sync running");
            seed_room_cache(client, room_cache).await;
            if !room_cache.is_empty() {
                emit_room_update(room_cache, on_sync);
            }
            on_sync(SyncEvent::Connected);
            LoopAction::Continue
        }
        SyncState::Error(err) => {
            let msg = err.to_string();
            tracing::warn!("sliding sync error: {msg}");
            if is_auth_error(&err) {
                on_sync(SyncEvent::SessionExpired);
                return LoopAction::Break;
            }
            on_sync(SyncEvent::ConnectionError(msg));
            sync_service.start().await;
            LoopAction::Continue
        }
        SyncState::Terminated => {
            tracing::info!("sliding sync terminated");
            LoopAction::Break
        }
        SyncState::Idle | SyncState::Offline => LoopAction::Continue,
    }
}

async fn run_sync_loop(
    client: &Client,
    sync_service: &SyncService,
    room_updates_rx: &mut Receiver<RoomUpdates>,
    on_sync: &Arc<dyn Fn(SyncEvent) + Send + Sync>,
) {
    let mut room_cache: HashMap<String, DomainRoom> = HashMap::new();
    let mut state_stream = sync_service.state();

    seed_room_cache(client, &mut room_cache).await;
    if !room_cache.is_empty() {
        emit_room_update(&room_cache, on_sync);
    }
    on_sync(SyncEvent::Connected);

    loop {
        tokio::select! {
            biased;
            update = room_updates_rx.recv() => {
                if matches!(
                    handle_room_update(client, update, &mut room_cache, on_sync).await,
                    LoopAction::Break
                ) {
                    break;
                }
            }
            Some(state) = state_stream.next() => {
                if matches!(
                    handle_sync_state(client, state, sync_service, &mut room_cache, on_sync).await,
                    LoopAction::Break
                ) {
                    break;
                }
            }
        }
    }
}

pub(super) async fn start_sync(
    client: &Client,
    on_sync: Arc<dyn Fn(SyncEvent) + Send + Sync>,
) -> AppResult<()> {
    let sync_service = build_sync_service(client).await?;
    let mut room_updates_rx = client.subscribe_to_all_room_updates();

    sync_service.start().await;
    tracing::info!("sliding sync service started");

    run_sync_loop(client, &sync_service, &mut room_updates_rx, &on_sync).await;

    sync_service.stop().await;
    Ok(())
}
