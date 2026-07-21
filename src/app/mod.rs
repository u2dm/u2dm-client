mod active_timeline;
mod lifecycle;
mod media;
mod room_directory;
mod selection;
mod session;
mod task_group;
mod verification;

use std::sync::Arc;
use std::time::Duration;

use active_timeline::ActiveTimeline;
use lifecycle::Lifecycle;
use media::MediaActions;
use room_directory::RoomDirectory;
use selection::Selection;
use session::{AuthOutcome, SessionController};
use task_group::TaskGroup;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;
use verification::VerificationController;

use crate::commands::{
    AppViewState, DirectoryUpdate, Effect, LoginStep, UiCommand, ViewportChanged,
};
use crate::domain::models::{ConnectionStatus, Room, RoomId, Space};
use crate::ports::browser::BrowserPort;
use crate::ports::matrix::{AuthPort, AuthenticatedSession};
use crate::ports::media::MediaFilePort;
use crate::ports::output::AppOutputPort;
use crate::ports::storage::StoragePort;

pub struct AppService {
    storage: Arc<dyn StoragePort>,
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    dir_in_tx: mpsc::UnboundedSender<DirectoryUpdate>,
    output: Arc<dyn AppOutputPort>,
    background: TaskGroup,
    operations: TaskGroup,
    session: SessionController,
    room_directory: RoomDirectory,
    active_timeline: ActiveTimeline,
    verification: VerificationController,
    media: MediaActions,
    selection: Selection,
    lifecycle: Lifecycle,
    active: Option<AuthenticatedSession>,
    auth_rx: Option<mpsc::UnboundedReceiver<AuthOutcome>>,
    space_order_tx: watch::Sender<Option<Vec<String>>>,
    space_order_cancel: CancellationToken,
    space_order_handle: Option<JoinHandle<()>>,
}

const SPACE_ORDER_DEBOUNCE: Duration = Duration::from_millis(500);
const SPACE_ORDER_FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

impl AppService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        auth: Arc<dyn AuthPort>,
        storage: Arc<dyn StoragePort>,
        media_files: Arc<dyn MediaFilePort>,
        browser: Arc<dyn BrowserPort>,
        cmd_tx: mpsc::UnboundedSender<UiCommand>,
        dir_in_tx: mpsc::UnboundedSender<DirectoryUpdate>,
        output: Arc<dyn AppOutputPort>,
    ) -> Self {
        let (space_order_tx, space_order_rx) = watch::channel::<Option<Vec<String>>>(None);
        let space_order_cancel = CancellationToken::new();
        let space_order_handle = tokio::spawn(persist_space_order_task(
            Arc::clone(&storage),
            space_order_rx,
            space_order_cancel.clone(),
        ));
        let lifecycle = Lifecycle::new();
        let (auth_tx, auth_rx) = mpsc::unbounded_channel::<AuthOutcome>();
        Self {
            session: SessionController::new(
                auth,
                Arc::clone(&storage),
                browser,
                Arc::clone(&output),
                lifecycle.clone(),
                auth_tx,
            ),
            room_directory: RoomDirectory::new(Arc::clone(&output)),
            active_timeline: ActiveTimeline::new(cmd_tx.clone(), Arc::clone(&output)),
            verification: VerificationController::new(Arc::clone(&output)),
            media: MediaActions::new(media_files, Arc::clone(&output)),
            storage,
            cmd_tx,
            dir_in_tx,
            output,
            background: TaskGroup::new("background"),
            operations: TaskGroup::new("operations"),
            selection: Selection::default(),
            lifecycle,
            active: None,
            auth_rx: Some(auth_rx),
            space_order_tx,
            space_order_cancel,
            space_order_handle: Some(space_order_handle),
        }
    }

    pub async fn run(
        &mut self,
        mut cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
        mut dir_in_rx: mpsc::UnboundedReceiver<DirectoryUpdate>,
        mut scroll_in_rx: watch::Receiver<ViewportChanged>,
    ) {
        let Some(mut auth_rx) = self.auth_rx.take() else {
            return;
        };
        let mut dir_done = false;
        let mut scroll_done = false;
        let mut auth_done = false;
        loop {
            tokio::select! {
                maybe_cmd = cmd_rx.recv() => {
                    let Some(cmd) = maybe_cmd else { break };
                    Self::log_command(&cmd);
                    if self.dispatch(cmd).await {
                        break;
                    }
                }
                maybe_outcome = auth_rx.recv(), if !auth_done => {
                    match maybe_outcome {
                        Some(outcome) => self.complete_auth(outcome).await,
                        None => auth_done = true,
                    }
                }
                maybe_dir = dir_in_rx.recv(), if !dir_done => {
                    match maybe_dir {
                        Some(DirectoryUpdate::Rooms(rooms)) => {
                            self.handle_rooms_updated(rooms).await;
                        }
                        Some(DirectoryUpdate::Spaces(spaces)) => {
                            self.handle_spaces_updated(spaces);
                        }
                        None => dir_done = true,
                    }
                }
                changed = scroll_in_rx.changed(), if !scroll_done => {
                    if changed.is_err() {
                        scroll_done = true;
                    } else {
                        let viewport = scroll_in_rx.borrow_and_update().clone();
                        self.active_timeline.scroll_position_changed(
                            &viewport.room_id,
                            viewport.generation,
                            viewport.at_top,
                            viewport.at_bottom,
                        );
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn dispatch(&mut self, cmd: UiCommand) -> bool {
        let phase = self.lifecycle.phase();
        if !lifecycle::command_allowed(phase, &cmd) {
            tracing::debug!(?phase, command = %cmd, "rejecting command illegal in current phase");
            return false;
        }
        match cmd {
            UiCommand::RestoreSession => {
                self.session.spawn_restore_session(&mut self.operations);
            }
            UiCommand::CheckServer(homeserver) => {
                let attempt = self.lifecycle.begin_auth();
                self.session
                    .spawn_check_server(&mut self.operations, homeserver, attempt);
            }
            UiCommand::LoginPassword(creds) => {
                let attempt = self.lifecycle.begin_auth();
                self.session
                    .spawn_login_password(&mut self.operations, creds, attempt);
            }
            UiCommand::LoginOAuth => {
                let attempt = self.lifecycle.begin_auth();
                self.session
                    .spawn_login_oauth(&mut self.operations, attempt);
            }
            UiCommand::CancelOAuth => {
                if self.lifecycle.cancel_auth() {
                    self.session.cancel_oauth();
                }
            }
            UiCommand::FetchRooms => {
                self.handle_fetch_rooms().await;
            }
            UiCommand::SelectSpace(space) => {
                self.handle_select_space(space);
            }
            UiCommand::SelectSubspace(subspace) => {
                self.handle_select_subspace(subspace);
            }
            UiCommand::MoveSpace { from, to } => {
                if let Some(order) = self.room_directory.move_space(from, to) {
                    self.persist_space_order(order);
                }
            }
            UiCommand::SelectRoom(room_id) => {
                self.select_room(room_id).await;
            }
            UiCommand::RetryTimeline => {
                self.retry_timeline().await;
            }
            UiCommand::SendMessage {
                room_id,
                body,
                reply_to,
            } => {
                self.send_message(room_id, body, reply_to);
            }
            UiCommand::PaginateBackwards {
                room_id,
                generation,
            } => {
                self.active_timeline
                    .paginate_backwards(&room_id, generation);
            }
            UiCommand::PaginateForwards {
                room_id,
                generation,
            } => {
                self.active_timeline.paginate_forwards(&room_id, generation);
            }
            UiCommand::TimelinePaginationCompleted {
                room_id,
                generation,
                direction,
                outcome,
            } => {
                self.active_timeline
                    .complete_pagination(&room_id, generation, direction, outcome)
                    .await;
            }
            UiCommand::JumpToLatest {
                room_id,
                generation,
            } => {
                self.active_timeline.jump_to_latest(&room_id, generation);
            }
            UiCommand::OpenMedia { event_id } => {
                self.open_media(event_id);
            }
            UiCommand::SaveFile { event_id, filename } => {
                self.save_file(event_id, filename);
            }
            UiCommand::AcceptVerification => {
                self.accept_verification();
            }
            UiCommand::RejectVerification => {
                self.reject_verification();
            }
            UiCommand::ConfirmVerification => {
                self.confirm_verification();
            }
            UiCommand::SessionExpired => {
                self.handle_session_expired().await;
            }
            UiCommand::Logout => {
                self.handle_logout().await;
            }
            UiCommand::Quit => {
                self.handle_quit().await;
                return true;
            }
        }
        false
    }

    fn set_selected_space(&self, id: String) {
        self.output
            .publish(Box::new(move |view| view.directory.space_id = id));
    }

    fn set_selected_subspace(&self, id: String) {
        self.output
            .publish(Box::new(move |view| view.directory.subspace_id = id));
    }

    fn set_connection(&self, status: ConnectionStatus) {
        self.output
            .publish(Box::new(move |view| view.connection = status));
    }

    fn emit_login_success(&self, user_id: String) {
        self.output.publish(Box::new(move |view| {
            view.lifecycle.user_id = user_id;
            view.lifecycle.step = LoginStep::LoggedIn;
        }));
        self.output.emit_now(Effect::Status(String::new()));
    }

    async fn emit_selected_room(
        &self,
        id: RoomId,
        name: String,
        member_count: u64,
        generation: i32,
    ) {
        self.output
            .emit(Effect::SelectedRoom {
                id,
                name,
                member_count,
                generation,
            })
            .await;
    }

    fn persist_space_order(&self, order: Vec<String>) {
        if self.space_order_tx.send(Some(order)).is_err() {
            tracing::warn!("space order persister stopped; order not saved");
        }
    }

    fn log_command(cmd: &UiCommand) {
        tracing::info!(command = %cmd, "handling command");
    }

    async fn handle_rooms_updated(&mut self, rooms: Arc<[Room]>) {
        if self.active.is_none() {
            return;
        }
        if self.room_directory.store_rooms(rooms) {
            self.refresh_selected_room().await;
            self.room_directory.emit_directory(&self.selection);
        }
    }

    fn handle_spaces_updated(&mut self, spaces: Arc<[Space]>) {
        if self.active.is_none() {
            return;
        }
        if self.room_directory.store_spaces(spaces) {
            let outcome = self.room_directory.reconcile(&mut self.selection);
            if outcome.space_dropped {
                self.set_selected_space(String::new());
                self.set_selected_subspace(String::new());
            } else if outcome.subspace_dropped {
                self.set_selected_subspace(String::new());
            }
            self.room_directory.emit_directory(&self.selection);
        }
    }

    fn handle_select_space(&mut self, space: Option<RoomId>) {
        self.selection.set_space(space);
        self.set_selected_space(self.selection.space_id_str());
        self.set_selected_subspace(self.selection.subspace_id_str());
        self.room_directory.emit_subspaces(&self.selection);
        self.room_directory.emit_rooms(&self.selection);
    }

    fn handle_select_subspace(&mut self, subspace: Option<RoomId>) {
        self.selection.set_subspace(subspace);
        self.set_selected_subspace(self.selection.subspace_id_str());
        self.room_directory.emit_rooms(&self.selection);
    }

    async fn complete_auth(&mut self, outcome: AuthOutcome) {
        let Some(capability) = self.settle_auth_outcome(outcome).await else {
            tracing::info!("authentication superseded, dropping session");
            return;
        };
        let user_id = capability.session.user_id.clone();
        tracing::info!(%user_id, "authenticated");
        self.active = Some(capability);
        self.emit_login_success(user_id);
        if let Err(e) = self.cmd_tx.send(UiCommand::FetchRooms) {
            tracing::warn!("failed to trigger room fetch: {e}");
        }
    }

    async fn settle_auth_outcome(&mut self, outcome: AuthOutcome) -> Option<AuthenticatedSession> {
        match outcome {
            AuthOutcome::Login { attempt, session } => {
                self.lifecycle.promote_to_syncing(attempt)?;
                self.session.save_session(&session.session).await;
                Some(session)
            }
            AuthOutcome::Restore(session) => {
                self.lifecycle.restore_succeeded()?;
                Some(session)
            }
        }
    }

    fn send_message(&mut self, room_id: RoomId, body: String, reply_to: Option<String>) {
        let Some(timeline) = self.active.as_ref().map(|a| Arc::clone(&a.timeline)) else {
            return;
        };
        self.active_timeline
            .spawn_send(&mut self.operations, timeline, room_id, body, reply_to);
    }

    fn open_media(&mut self, event_id: String) {
        if let Some(media) = self.active.as_ref().map(|a| Arc::clone(&a.media)) {
            self.media.open_media(media, event_id);
        }
    }

    fn save_file(&mut self, event_id: String, filename: String) {
        if let Some(media) = self.active.as_ref().map(|a| Arc::clone(&a.media)) {
            self.media.save_file(media, event_id, filename);
        }
    }

    fn accept_verification(&mut self) {
        if let Some(verification) = self.active.as_ref().map(|a| Arc::clone(&a.verification)) {
            self.verification
                .spawn_accept(&mut self.operations, verification);
        }
    }

    fn reject_verification(&mut self) {
        if let Some(verification) = self.active.as_ref().map(|a| Arc::clone(&a.verification)) {
            self.verification
                .spawn_reject(&mut self.operations, verification);
        }
    }

    fn confirm_verification(&mut self) {
        if let Some(verification) = self.active.as_ref().map(|a| Arc::clone(&a.verification)) {
            self.verification
                .spawn_confirm(&mut self.operations, verification);
        }
    }

    async fn select_room(&mut self, room_id: RoomId) {
        self.selection.room = Some(room_id.clone());
        let generation = self.selection.next_generation();
        let (name, member_count) = self
            .room_directory
            .selected_room_meta(&self.selection)
            .map_or_else(|| (String::new(), 0), |m| (m.name, m.member_count));
        self.emit_selected_room(room_id.clone(), name, member_count, generation)
            .await;
        let Some(timeline) = self.active.as_ref().map(|a| Arc::clone(&a.timeline)) else {
            return;
        };
        self.active_timeline
            .select_room(timeline, room_id, generation)
            .await;
    }

    async fn retry_timeline(&mut self) {
        let Some(room_id) = self.selection.room.clone() else {
            return;
        };
        self.select_room(room_id).await;
    }

    async fn refresh_selected_room(&mut self) {
        let Some(room_id) = self.selection.room.clone() else {
            return;
        };
        let generation = self.selection.generation;
        if let Some(meta) = self.room_directory.selected_room_meta(&self.selection) {
            self.emit_selected_room(room_id, meta.name, meta.member_count, generation)
                .await;
        } else {
            self.selection.room = None;
            self.emit_selected_room(RoomId::new(String::new()), String::new(), 0, generation)
                .await;
        }
    }

    async fn handle_fetch_rooms(&mut self) {
        let Some((sync, verification, lifecycle_port)) = self.active.as_ref().map(|a| {
            (
                Arc::clone(&a.sync),
                Arc::clone(&a.verification),
                Arc::clone(&a.lifecycle),
            )
        }) else {
            tracing::debug!("fetch rooms without an authenticated session, ignoring");
            return;
        };
        self.room_directory.connect(self.storage.as_ref()).await;
        self.output.emit_now(Effect::Status("syncing".into()));
        self.background.restart().await;
        self.session
            .spawn_session_persister(&mut self.background, Arc::clone(&lifecycle_port));
        self.verification
            .spawn_forwarder(&mut self.background, verification);
        self.set_connection(ConnectionStatus::Connecting);
        RoomDirectory::spawn_sync_pipeline(
            &mut self.background,
            sync,
            Arc::clone(&self.output),
            self.cmd_tx.clone(),
            self.dir_in_tx.clone(),
        );
        self.session
            .spawn_user_avatar_fetch(&mut self.background, lifecycle_port);
    }

    async fn shutdown_all_tasks(&mut self) {
        tokio::join!(
            self.background.shutdown(),
            self.active_timeline.shutdown(),
            self.operations.restart(),
            self.media.cancel_and_drain(),
        );
    }

    async fn handle_session_expired(&mut self) {
        let Some(session) = self.lifecycle.begin_logout() else {
            return;
        };
        tracing::info!("session expired, clearing local state");
        self.output.replace(AppViewState::logged_out());
        self.output.emit(Effect::LoggedOut).await;
        self.shutdown_all_tasks().await;
        self.room_directory.reset();
        self.selection = Selection::default();
        self.active = None;
        self.session
            .spawn_expire_session(&mut self.operations, session);
    }

    async fn handle_logout(&mut self) {
        let Some(session) = self.lifecycle.begin_logout() else {
            return;
        };
        let lifecycle_port = self.active.as_ref().map(|a| Arc::clone(&a.lifecycle));
        self.output.replace(AppViewState::logged_out());
        self.output.emit(Effect::LoggedOut).await;
        self.shutdown_all_tasks().await;
        self.room_directory.reset();
        self.selection = Selection::default();
        self.active = None;
        match lifecycle_port {
            Some(port) => self
                .session
                .spawn_logout(&mut self.operations, session, port),
            None => {
                self.lifecycle.finish_logout(session);
            }
        }
    }

    async fn handle_quit(&mut self) {
        tokio::join!(
            self.background.shutdown(),
            self.active_timeline.shutdown(),
            self.operations.shutdown(),
            self.media.drain(),
        );
        self.flush_space_order().await;
    }

    async fn flush_space_order(&mut self) {
        self.space_order_cancel.cancel();
        let Some(handle) = self.space_order_handle.take() else {
            return;
        };
        if timeout(SPACE_ORDER_FLUSH_TIMEOUT, handle).await.is_err() {
            tracing::warn!("timed out flushing space order on quit");
        }
    }
}

async fn persist_space_order_task(
    storage: Arc<dyn StoragePort>,
    mut rx: watch::Receiver<Option<Vec<String>>>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            changed = rx.changed() => {
                if changed.is_err() {
                    break;
                }
            }
        }

        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            () = sleep(SPACE_ORDER_DEBOUNCE) => {}
        }

        let order = rx.borrow_and_update().clone();
        save_space_order(&storage, order).await;
    }

    let pending = rx.borrow().clone();
    save_space_order(&storage, pending).await;
}

async fn save_space_order(storage: &Arc<dyn StoragePort>, order: Option<Vec<String>>) {
    if let Some(order) = order
        && let Err(e) = storage.save_space_order(&order).await
    {
        tracing::warn!("failed to persist space order: {e}");
    }
}
