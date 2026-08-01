mod avatars;
mod build;
mod directory;
mod health;

use std::future;
use std::sync::Arc;

use matrix_sdk::Client;
use matrix_sdk::notification_settings::NotificationSettings;
use matrix_sdk::sync::RoomUpdates;
use matrix_sdk_base::RoomInfoNotableUpdate;
use matrix_sdk_ui::sync_service::{State as SyncState, SyncService};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, sleep_until};
use tokio_util::sync::CancellationToken;

use self::avatars::AvatarFetcher;
use self::directory::Directory;
use self::health::{SyncHealth, is_auth_error};
use super::media::MediaService;
use crate::domain::models::{SyncEvent, SyncOutcome};
use crate::error::{AppError, Result as AppResult};
use crate::ports::matrix::SyncSink as OnSync;

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

enum LoopAction {
    Continue,
    Terminal(SyncOutcome),
}

async fn resync(client: &Client, dir: &mut Directory) {
    dir.seed(client).await;
    dir.mark_rooms();
    dir.mark_spaces();
}

async fn handle_room_update(
    client: &Client,
    update: Result<RoomUpdates, RecvError>,
    dir: &mut Directory,
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
            dir.note_room_updates(client, &updates);
            LoopAction::Continue
        }
        Err(RecvError::Lagged(n)) => {
            tracing::warn!("room updates lagged by {n} messages, full rebuild");
            resync(client, dir).await;
            LoopAction::Continue
        }
        Err(RecvError::Closed) => LoopAction::Terminal(SyncOutcome::Recoverable(
            "room updates channel closed".into(),
        )),
    }
}

async fn handle_room_info_update(
    client: &Client,
    update: Result<RoomInfoNotableUpdate, RecvError>,
    dir: &mut Directory,
) -> LoopAction {
    match update {
        Ok(update) => {
            dir.note_room_info(client, &update);
            LoopAction::Continue
        }
        Err(RecvError::Lagged(n)) => {
            tracing::warn!("room info updates lagged by {n} messages, full rebuild");
            resync(client, dir).await;
            LoopAction::Continue
        }
        Err(RecvError::Closed) => {
            LoopAction::Terminal(SyncOutcome::Recoverable("room info channel closed".into()))
        }
    }
}

#[allow(clippy::cognitive_complexity)]
async fn handle_sync_state(
    client: &Client,
    state: SyncState,
    dir: &mut Directory,
    health: &mut SyncHealth,
    on_sync: &OnSync,
) -> LoopAction {
    match state {
        SyncState::Running => {
            if health.on_running() {
                tracing::info!("sliding sync reconnected");
                resync(client, dir).await;
            }
            if health.should_announce_connected() {
                on_sync(SyncEvent::Connected);
            }
            LoopAction::Continue
        }
        SyncState::Error(err) => {
            let msg = err.to_string();
            if is_auth_error(&err) {
                tracing::warn!("sliding sync error: {msg}");
                return LoopAction::Terminal(SyncOutcome::SessionExpired);
            }
            let delay = health.on_error();
            tracing::warn!("sliding sync error, restarting in {delay:?}: {msg}");
            on_sync(SyncEvent::ConnectionError(msg));
            LoopAction::Continue
        }
        SyncState::Terminated => {
            tracing::info!("sliding sync terminated");
            LoopAction::Terminal(SyncOutcome::Recoverable("sliding sync terminated".into()))
        }
        SyncState::Offline => {
            health.on_offline();
            LoopAction::Continue
        }
        SyncState::Idle => LoopAction::Continue,
    }
}

fn handle_push_rules_change(changed: &Result<(), RecvError>, dir: &mut Directory) -> LoopAction {
    match changed {
        Ok(()) | Err(RecvError::Lagged(_)) => {
            dir.mark_all_flags();
            LoopAction::Continue
        }
        Err(RecvError::Closed) => {
            LoopAction::Terminal(SyncOutcome::Recoverable("push rules channel closed".into()))
        }
    }
}

async fn restart_sync(sync_service: &SyncService, health: &mut SyncHealth) -> LoopAction {
    health.on_restart();
    tracing::info!("restarting sliding sync");
    sync_service.start().await;
    LoopAction::Continue
}

async fn wait_until(at: Option<Instant>) {
    match at {
        Some(at) => sleep_until(at).await,
        None => future::pending::<()>().await,
    }
}

async fn run_sync_loop(
    client: &Client,
    sync_service: &SyncService,
    room_updates_rx: &mut Receiver<RoomUpdates>,
    push_rules_rx: &mut Receiver<()>,
    notifications: NotificationSettings,
    on_sync: &OnSync,
    avatars: &mut AvatarFetcher,
) -> SyncOutcome {
    let mut dir = Directory::new(notifications);
    let mut health = SyncHealth::started();
    let mut state_stream = sync_service.state();
    let mut room_info_rx = client.room_info_notable_update_receiver();

    resync(client, &mut dir).await;
    dir.flush(client, on_sync, avatars).await;
    on_sync(SyncEvent::Connected);

    loop {
        let flush_fut = wait_until(dir.flush_at());
        let retry_fut = wait_until(avatars.due_at());
        let restart_fut = wait_until(health.restart_at());
        let action = tokio::select! {
            biased;
            state = state_stream.next() => match state {
                Some(state) => handle_sync_state(client, state, &mut dir, &mut health, on_sync).await,
                None => LoopAction::Terminal(SyncOutcome::Recoverable("sync state stream ended".into())),
            },
            () = restart_fut => restart_sync(sync_service, &mut health).await,
            () = flush_fut => {
                dir.flush(client, on_sync, avatars).await;
                LoopAction::Continue
            }
            () = retry_fut => {
                avatars.wake(client);
                LoopAction::Continue
            }
            Some(joined) = avatars.join_next() => {
                if let Some(kind) = avatars.finish(client, joined) {
                    dir.mark_kind(kind);
                }
                LoopAction::Continue
            }
            update = room_updates_rx.recv() => {
                handle_room_update(client, update, &mut dir).await
            }
            info = room_info_rx.recv() => {
                handle_room_info_update(client, info, &mut dir).await
            }
            changed = push_rules_rx.recv() => {
                handle_push_rules_change(&changed, &mut dir)
            }
        };
        if let LoopAction::Terminal(outcome) = action {
            return outcome;
        }
    }
}

pub(super) async fn start_sync(
    client: &Client,
    media: Arc<MediaService>,
    on_sync: OnSync,
    cancel: CancellationToken,
) -> SyncOutcome {
    let sync_service = match build_sync_service(client).await {
        Ok(service) => service,
        Err(e) => return SyncOutcome::Fatal(format!("failed to build sync service: {e}")),
    };
    let mut room_updates_rx = client.subscribe_to_all_room_updates();
    let mut avatars = AvatarFetcher::new(media);
    let notifications = client.notification_settings().await;
    let mut push_rules_rx = notifications.subscribe_to_changes();

    sync_service.start().await;
    tracing::info!("sliding sync service started");

    let outcome = tokio::select! {
        outcome = run_sync_loop(
            client,
            &sync_service,
            &mut room_updates_rx,
            &mut push_rules_rx,
            notifications,
            &on_sync,
            &mut avatars,
        ) => outcome,
        () = cancel.cancelled() => {
            tracing::debug!("sync cancelled, stopping sync service");
            SyncOutcome::Cancelled
        }
    };

    sync_service.stop().await;
    outcome
}
