mod avatars;
mod build;
mod directory;
mod health;
mod hierarchy;
mod info;
mod invites;
mod push;
mod send_queue;
mod typing;
mod upgrades;

use std::future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use matrix_sdk::Client;
use matrix_sdk::notification_settings::NotificationSettings;
use matrix_sdk::ruma::events::space_order::SpaceOrderEventContent;
use matrix_sdk::ruma::{OwnedRoomId, SpaceChildOrder};
use matrix_sdk::send_queue::SendQueueRoomError;
use matrix_sdk::sync::RoomUpdates;
use matrix_sdk_base::RoomInfoNotableUpdate;
use matrix_sdk_ui::room_list_service::State as RoomListState;
use matrix_sdk_ui::sync_service::{Error as SyncServiceError, State as SyncState, SyncService};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;
use tokio::time::{Instant, sleep_until};
use tokio_util::sync::CancellationToken;

use self::avatars::AvatarFetcher;
use self::directory::Directory;
use self::health::{SyncHealth, endpoint_missing, session_loss};
pub(super) use self::hierarchy::MatrixSpaceIndex;
pub(super) use self::info::MatrixRoomInfo;
pub(super) use self::invites::MatrixInvites;
pub(super) use self::push::MatrixPush;
use self::send_queue::SendQueueRecovery;
pub(super) use self::typing::MatrixTyping;
pub(super) use self::upgrades::MatrixUpgrades;
use super::auth;
use super::media::MediaService;
use super::session::ClientHandle;
use crate::domain::room::RoomId;
use crate::domain::sync::{SyncEvent, SyncOutcome, SyncSupport};
use crate::error::{AppError, Result as AppResult};
use crate::ports::matrix::{SpaceOrderPort, SyncPort, SyncSink as OnSync};

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

type SelectedRoom = watch::Receiver<Option<RoomId>>;

async fn subscribe_selected_room(sync_service: &SyncService, room_id: Option<&RoomId>) {
    let subscribed: Option<OwnedRoomId> =
        room_id.and_then(|id| OwnedRoomId::try_from(id.as_ref()).ok());
    sync_service
        .room_list_service()
        .set_room_subscriptions(subscribed.as_deref().as_slice())
        .await;
    if let Some(room_id) = subscribed {
        tracing::debug!(%room_id, "subscribed the selected room");
    } else {
        tracing::debug!("no room is selected, so no room is subscribed");
    }
}

async fn start_with_selected_room(sync_service: &SyncService, selected: &mut SelectedRoom) {
    let room_id = selected.borrow_and_update().clone();
    subscribe_selected_room(sync_service, room_id.as_ref()).await;
    sync_service.start().await;
    tracing::info!("sliding sync service started");
}

async fn follow_selected_room(
    sync_service: &SyncService,
    mut selected: SelectedRoom,
) -> SyncOutcome {
    while selected.changed().await.is_ok() {
        let room_id = selected.borrow_and_update().clone();
        subscribe_selected_room(sync_service, room_id.as_ref()).await;
    }
    SyncOutcome::Recoverable("the selected room channel closed".into())
}

async fn resync(client: &Client, dir: &mut Directory) {
    dir.seed(client).await;
    dir.mark_rooms();
    dir.mark_invites();
    dir.mark_spaces();
}

async fn handle_room_update(
    client: &Client,
    update: Result<RoomUpdates, RecvError>,
    dir: &mut Directory,
) -> LoopAction {
    match update {
        Ok(updates) => {
            if updates.joined.is_empty() && updates.left.is_empty() && updates.invited.is_empty() {
                return LoopAction::Continue;
            }
            tracing::debug!(
                joined = updates.joined.len(),
                left = updates.left.len(),
                invited = updates.invited.len(),
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

async fn lacks_sliding_sync(client: &Client, err: &SyncServiceError) -> bool {
    if !endpoint_missing(err) {
        return false;
    }
    match auth::sync_support(client).await {
        Ok(support) => support == SyncSupport::ClassicOnly,
        Err(e) => {
            tracing::debug!("could not ask the homeserver whether it offers sliding sync: {e}");
            false
        }
    }
}

async fn report_sync_error(
    client: &Client,
    err: &SyncServiceError,
    delay: Duration,
    on_sync: &OnSync,
) {
    let msg = err.to_string();
    if lacks_sliding_sync(client, err).await {
        tracing::warn!(
            "the homeserver does not offer sliding sync, restarting in {delay:?}: {msg}"
        );
        on_sync(SyncEvent::SlidingSyncUnsupported);
    } else {
        tracing::warn!("sliding sync error, restarting in {delay:?}: {msg}");
        on_sync(SyncEvent::ConnectionError(msg));
    }
}

async fn handle_sync_error(
    client: &Client,
    err: &SyncServiceError,
    health: &mut SyncHealth,
    on_sync: &OnSync,
) -> LoopAction {
    if let Some(loss) = session_loss(err) {
        tracing::warn!(?loss, "sliding sync error: {err}");
        return LoopAction::Terminal(SyncOutcome::SessionLost(loss));
    }
    let delay = health.on_error();
    report_sync_error(client, err, delay, on_sync).await;
    LoopAction::Continue
}

async fn handle_sync_state(
    client: &Client,
    state: Option<SyncState>,
    dir: &mut Directory,
    health: &mut SyncHealth,
    sends: &mut SendQueueRecovery,
    on_sync: &OnSync,
) -> LoopAction {
    match state {
        Some(SyncState::Running) => {
            if health.on_running() {
                tracing::info!("sliding sync restarted");
                resync(client, dir).await;
                sends.resume(client).await;
            }
            LoopAction::Continue
        }
        Some(SyncState::Error(err)) => handle_sync_error(client, &err, health, on_sync).await,
        Some(SyncState::Terminated) => {
            tracing::info!("sliding sync terminated");
            LoopAction::Terminal(SyncOutcome::Recoverable("sliding sync terminated".into()))
        }
        Some(SyncState::Offline) => {
            health.on_offline();
            LoopAction::Continue
        }
        Some(SyncState::Idle) => LoopAction::Continue,
        None => LoopAction::Terminal(SyncOutcome::Recoverable("sync state stream ended".into())),
    }
}

fn set_by_a_response(state: &RoomListState) -> bool {
    matches!(
        state,
        RoomListState::SettingUp | RoomListState::Running | RoomListState::Recovering
    )
}

fn handle_room_list_state(
    state: Option<RoomListState>,
    health: &mut SyncHealth,
    on_sync: &OnSync,
) -> LoopAction {
    let Some(state) = state else {
        return LoopAction::Terminal(SyncOutcome::Recoverable(
            "room list state stream ended".into(),
        ));
    };
    if set_by_a_response(&state) && health.on_response() {
        tracing::info!("the homeserver answered sliding sync");
        on_sync(SyncEvent::Connected);
    }
    LoopAction::Continue
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

fn handle_send_queue_error(
    update: Result<SendQueueRoomError, RecvError>,
    sends: &mut SendQueueRecovery,
) -> LoopAction {
    match update {
        Ok(failure) => {
            sends.on_error(&failure);
            LoopAction::Continue
        }
        Err(RecvError::Lagged(n)) => {
            sends.on_lagged(n);
            LoopAction::Continue
        }
        Err(RecvError::Closed) => LoopAction::Terminal(SyncOutcome::Recoverable(
            "send queue error channel closed".into(),
        )),
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
    let mut room_list_states = sync_service.room_list_service().state();
    room_list_states.reset();
    let mut room_info_rx = client.room_info_notable_update_receiver();
    let mut sends = SendQueueRecovery::start(client).await;

    resync(client, &mut dir).await;
    dir.flush(client, on_sync, avatars).await;

    loop {
        let flush_fut = wait_until(dir.flush_at());
        let retry_fut = wait_until(avatars.due_at());
        let restart_fut = wait_until(health.restart_at());
        let resume_fut = wait_until(sends.resume_at());
        let action = tokio::select! {
            biased;
            state = state_stream.next() => {
                handle_sync_state(client, state, &mut dir, &mut health, &mut sends, on_sync).await
            }
            state = room_list_states.next() => handle_room_list_state(state, &mut health, on_sync),
            () = restart_fut => restart_sync(sync_service, &mut health).await,
            () = resume_fut => {
                sends.resume(client).await;
                LoopAction::Continue
            }
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
            failure = sends.next_error() => {
                handle_send_queue_error(failure, &mut sends)
            }
        };
        if let LoopAction::Terminal(outcome) = action {
            return outcome;
        }
    }
}

async fn drive_sync_service(
    client: &Client,
    media: Arc<MediaService>,
    on_sync: OnSync,
    mut selected: SelectedRoom,
    cancel: CancellationToken,
) -> SyncOutcome {
    let sync_service = match build_sync_service(client).await {
        Ok(service) => service,
        Err(e) => return SyncOutcome::Fatal(format!("failed to build sync service: {e}")),
    };
    client.send_queue().enable_upload_progress(true);
    let mut room_updates_rx = client.subscribe_to_all_room_updates();
    let mut avatars = AvatarFetcher::new(media);
    let notifications = client.notification_settings().await;
    let mut push_rules_rx = notifications.subscribe_to_changes();

    start_with_selected_room(&sync_service, &mut selected).await;

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
        outcome = follow_selected_room(&sync_service, selected) => outcome,
        () = cancel.cancelled() => {
            tracing::debug!("sync cancelled, stopping sync service");
            SyncOutcome::Cancelled
        }
    };

    sync_service.stop().await;
    outcome
}

pub(super) struct MatrixSync {
    matrix: Arc<ClientHandle>,
    selected: watch::Sender<Option<RoomId>>,
}

impl MatrixSync {
    pub(super) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self {
            matrix,
            selected: watch::Sender::new(None),
        }
    }
}

#[async_trait]
impl SyncPort for MatrixSync {
    async fn start_sync(&self, on_sync: OnSync, cancel: CancellationToken) -> SyncOutcome {
        tracing::info!("starting continuous sync loop");
        let client = match self.matrix.client().await {
            Ok(client) => client,
            Err(e) => return SyncOutcome::Fatal(e.to_string()),
        };
        drive_sync_service(
            &client,
            Arc::clone(self.matrix.media()),
            on_sync,
            self.selected.subscribe(),
            cancel,
        )
        .await
    }

    fn set_selected_room(&self, room_id: Option<&RoomId>) {
        self.selected.send_if_modified(|selected| {
            if selected.as_ref() == room_id {
                return false;
            }
            *selected = room_id.cloned();
            true
        });
    }
}

pub(super) struct MatrixSpaceOrder {
    matrix: Arc<ClientHandle>,
}

impl MatrixSpaceOrder {
    pub(super) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self { matrix }
    }
}

#[async_trait]
impl SpaceOrderPort for MatrixSpaceOrder {
    async fn set_space_order(&self, space_id: &RoomId, order: &str) -> AppResult<()> {
        let room = self.matrix.room(space_id).await?;
        let order = SpaceChildOrder::parse(order).map_err(|e| AppError::Other(e.to_string()))?;
        room.set_account_data(SpaceOrderEventContent::new(order))
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        Ok(())
    }
}
