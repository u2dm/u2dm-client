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
use session::SessionController;
use task_group::TaskGroup;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;
use verification::VerificationController;

use crate::commands::{UiCommand, ViewportChanged};
use crate::domain::models::{ConnectionStatus, Room, RoomId, Space};
use crate::ports::browser::BrowserPort;
use crate::ports::matrix::MatrixPort;
use crate::ports::media::MediaFilePort;
use crate::ports::output::AppOutputPort;
use crate::ports::storage::StoragePort;

pub struct AppService {
    matrix: Arc<dyn MatrixPort>,
    storage: Arc<dyn StoragePort>,
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    rooms_in_tx: watch::Sender<Arc<[Room]>>,
    spaces_in_tx: watch::Sender<Arc<[Space]>>,
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
    space_order_tx: watch::Sender<Option<Vec<String>>>,
    space_order_cancel: CancellationToken,
    space_order_handle: Option<JoinHandle<()>>,
}

const SPACE_ORDER_DEBOUNCE: Duration = Duration::from_millis(500);
const SPACE_ORDER_FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

impl AppService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        matrix: Arc<dyn MatrixPort>,
        storage: Arc<dyn StoragePort>,
        media_files: Arc<dyn MediaFilePort>,
        browser: Arc<dyn BrowserPort>,
        cmd_tx: mpsc::UnboundedSender<UiCommand>,
        rooms_in_tx: watch::Sender<Arc<[Room]>>,
        spaces_in_tx: watch::Sender<Arc<[Space]>>,
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
        Self {
            session: SessionController::new(
                Arc::clone(&matrix),
                Arc::clone(&storage),
                browser,
                cmd_tx.clone(),
                Arc::clone(&output),
                lifecycle.clone(),
            ),
            room_directory: RoomDirectory::new(Arc::clone(&output)),
            active_timeline: ActiveTimeline::new(
                Arc::clone(&matrix),
                cmd_tx.clone(),
                Arc::clone(&output),
            ),
            verification: VerificationController::new(Arc::clone(&matrix), Arc::clone(&output)),
            media: MediaActions::new(Arc::clone(&matrix), media_files, Arc::clone(&output)),
            matrix,
            storage,
            cmd_tx,
            rooms_in_tx,
            spaces_in_tx,
            output,
            background: TaskGroup::new("background"),
            operations: TaskGroup::new("operations"),
            selection: Selection::default(),
            lifecycle,
            space_order_tx,
            space_order_cancel,
            space_order_handle: Some(space_order_handle),
        }
    }

    pub async fn run(
        &mut self,
        mut cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
        mut rooms_in_rx: watch::Receiver<Arc<[Room]>>,
        mut spaces_in_rx: watch::Receiver<Arc<[Space]>>,
        mut scroll_in_rx: watch::Receiver<ViewportChanged>,
    ) {
        let mut rooms_done = false;
        let mut spaces_done = false;
        let mut scroll_done = false;
        loop {
            tokio::select! {
                maybe_cmd = cmd_rx.recv() => {
                    let Some(cmd) = maybe_cmd else { break };
                    Self::log_command(&cmd);
                    if self.dispatch(cmd).await {
                        break;
                    }
                }
                changed = rooms_in_rx.changed(), if !rooms_done => {
                    if changed.is_err() {
                        rooms_done = true;
                    } else {
                        let rooms = rooms_in_rx.borrow_and_update().clone();
                        self.handle_rooms_updated(rooms).await;
                    }
                }
                changed = spaces_in_rx.changed(), if !spaces_done => {
                    if changed.is_err() {
                        spaces_done = true;
                    } else {
                        let spaces = spaces_in_rx.borrow_and_update().clone();
                        self.handle_spaces_updated(spaces).await;
                    }
                }
                changed = scroll_in_rx.changed(), if !scroll_done => {
                    if changed.is_err() {
                        scroll_done = true;
                    } else {
                        let viewport = scroll_in_rx.borrow_and_update().clone();
                        self.active_timeline
                            .scroll_position_changed(
                                &viewport.room_id,
                                viewport.generation,
                                viewport.at_top,
                                viewport.at_bottom,
                            )
                            .await;
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
                self.handle_select_space(space).await;
            }
            UiCommand::SelectSubspace(subspace) => {
                self.handle_select_subspace(subspace).await;
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
                self.active_timeline
                    .spawn_send(&mut self.operations, room_id, body, reply_to);
            }
            UiCommand::PaginateBackwards {
                room_id,
                generation,
            } => {
                self.active_timeline
                    .paginate_backwards(&room_id, generation)
                    .await;
            }
            UiCommand::PaginateForwards {
                room_id,
                generation,
            } => {
                self.active_timeline
                    .paginate_forwards(&room_id, generation)
                    .await;
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
                self.active_timeline
                    .jump_to_latest(&room_id, generation)
                    .await;
            }
            UiCommand::OpenMedia { event_id } => {
                self.media.open_media(event_id);
            }
            UiCommand::SaveFile { event_id, filename } => {
                self.media.save_file(event_id, filename);
            }
            UiCommand::AcceptVerification => {
                self.verification.spawn_accept(&mut self.operations);
            }
            UiCommand::RejectVerification => {
                self.verification.spawn_reject(&mut self.operations);
            }
            UiCommand::ConfirmVerification => {
                self.verification.spawn_confirm(&mut self.operations);
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

    fn persist_space_order(&self, order: Vec<String>) {
        if self.space_order_tx.send(Some(order)).is_err() {
            tracing::warn!("space order persister stopped; order not saved");
        }
    }

    fn log_command(cmd: &UiCommand) {
        tracing::info!(command = %cmd, "handling command");
    }

    async fn handle_rooms_updated(&mut self, rooms: Arc<[Room]>) {
        if self.room_directory.store_rooms(rooms) {
            self.refresh_selected_room().await;
            self.room_directory.emit_directory(&self.selection);
        }
    }

    async fn handle_spaces_updated(&mut self, spaces: Arc<[Space]>) {
        if self.room_directory.store_spaces(spaces) {
            let outcome = self.room_directory.reconcile(&mut self.selection);
            if outcome.space_dropped {
                self.output.selected_space(String::new()).await;
                self.output.selected_subspace(String::new()).await;
            } else if outcome.subspace_dropped {
                self.output.selected_subspace(String::new()).await;
            }
            self.room_directory.emit_directory(&self.selection);
        }
    }

    async fn handle_select_space(&mut self, space: Option<RoomId>) {
        self.selection.set_space(space);
        self.output
            .selected_space(self.selection.space_id_str())
            .await;
        self.output
            .selected_subspace(self.selection.subspace_id_str())
            .await;
        self.room_directory.emit_subspaces(&self.selection);
        self.room_directory.emit_rooms(&self.selection);
    }

    async fn handle_select_subspace(&mut self, subspace: Option<RoomId>) {
        self.selection.set_subspace(subspace);
        self.output
            .selected_subspace(self.selection.subspace_id_str())
            .await;
        self.room_directory.emit_rooms(&self.selection);
    }

    async fn select_room(&mut self, room_id: RoomId) {
        self.selection.room = Some(room_id.clone());
        let generation = self.selection.next_generation();
        let (name, member_count) = self
            .room_directory
            .selected_room_meta(&self.selection)
            .map_or_else(|| (String::new(), 0), |m| (m.name, m.member_count));
        self.output
            .selected_room(room_id.clone(), name, member_count, generation)
            .await;
        self.active_timeline.select_room(room_id, generation).await;
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
            self.output
                .selected_room(room_id, meta.name, meta.member_count, generation)
                .await;
        } else {
            self.selection.room = None;
            self.output
                .selected_room(RoomId::new(String::new()), String::new(), 0, generation)
                .await;
        }
    }

    async fn handle_fetch_rooms(&mut self) {
        self.room_directory.connect(self.storage.as_ref()).await;
        self.output.status("syncing".into());
        self.start_background_tasks().await;
        self.output.connection_status(ConnectionStatus::Connecting);
        RoomDirectory::spawn_sync_pipeline(
            &mut self.background,
            Arc::clone(&self.matrix),
            Arc::clone(&self.output),
            self.cmd_tx.clone(),
            self.rooms_in_tx.clone(),
            self.spaces_in_tx.clone(),
        );
        self.session.spawn_user_avatar_fetch(&mut self.background);
    }

    async fn start_background_tasks(&mut self) {
        self.background.restart().await;
        self.session.spawn_session_persister(&mut self.background);
        self.verification.spawn_forwarder(&mut self.background);
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
        self.output
            .connection_status(ConnectionStatus::Disconnected);
        self.output.logged_out().await;
        self.shutdown_all_tasks().await;
        self.room_directory.reset();
        self.selection = Selection::default();
        self.session
            .spawn_expire_session(&mut self.operations, session);
    }

    async fn handle_logout(&mut self) {
        let Some(session) = self.lifecycle.begin_logout() else {
            return;
        };
        self.output
            .connection_status(ConnectionStatus::Disconnected);
        self.output.logged_out().await;
        self.shutdown_all_tasks().await;
        self.room_directory.reset();
        self.selection = Selection::default();
        self.session.spawn_logout(&mut self.operations, session);
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
