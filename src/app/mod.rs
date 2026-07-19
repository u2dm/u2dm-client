mod active_timeline;
mod media;
mod room_directory;
mod selection;
mod session;
mod task_group;
mod verification;

use std::sync::Arc;

use active_timeline::ActiveTimeline;
use media::MediaActions;
use room_directory::RoomDirectory;
use selection::Selection;
use session::SessionController;
use task_group::TaskGroup;
use tokio::sync::mpsc;
use verification::VerificationController;

use crate::commands::UiCommand;
use crate::domain::models::{ConnectionStatus, Room, RoomId, Space};
use crate::ports::browser::BrowserPort;
use crate::ports::matrix::MatrixPort;
use crate::ports::media::MediaFilePort;
use crate::ports::output::AppOutputPort;
use crate::ports::storage::StoragePort;

pub struct AppService {
    matrix: Arc<dyn MatrixPort>,
    storage: Arc<dyn StoragePort>,
    cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    output: Arc<dyn AppOutputPort>,
    background: TaskGroup,
    operations: TaskGroup,
    session: SessionController,
    room_directory: RoomDirectory,
    active_timeline: ActiveTimeline,
    verification: VerificationController,
    media: MediaActions,
    selection: Selection,
}

impl AppService {
    pub fn new(
        matrix: Arc<dyn MatrixPort>,
        storage: Arc<dyn StoragePort>,
        media_files: Arc<dyn MediaFilePort>,
        browser: Arc<dyn BrowserPort>,
        cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
        cmd_tx: mpsc::UnboundedSender<UiCommand>,
        output: Arc<dyn AppOutputPort>,
    ) -> Self {
        Self {
            session: SessionController::new(
                Arc::clone(&matrix),
                Arc::clone(&storage),
                browser,
                cmd_tx.clone(),
                Arc::clone(&output),
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
            cmd_rx,
            cmd_tx,
            output,
            background: TaskGroup::new(),
            operations: TaskGroup::new(),
            selection: Selection::default(),
        }
    }

    pub async fn run(&mut self) {
        while let Some(cmd) = self.cmd_rx.recv().await {
            Self::log_command(&cmd);
            if self.dispatch(cmd).await {
                break;
            }
        }
    }

    async fn dispatch(&mut self, cmd: UiCommand) -> bool {
        match cmd {
            UiCommand::RestoreSession => {
                self.session.spawn_restore_session(&mut self.operations);
            }
            UiCommand::CheckServer(homeserver) => {
                self.session
                    .spawn_check_server(&mut self.operations, homeserver);
            }
            UiCommand::LoginPassword(creds) => {
                self.session
                    .spawn_login_password(&mut self.operations, creds);
            }
            UiCommand::LoginOAuth => {
                self.session.spawn_login_oauth(&mut self.operations);
            }
            UiCommand::FetchRooms => {
                self.handle_fetch_rooms().await;
            }
            UiCommand::RoomsUpdated(rooms) => {
                self.handle_rooms_updated(rooms);
            }
            UiCommand::SpacesUpdated(spaces) => {
                self.handle_spaces_updated(spaces);
            }
            UiCommand::SelectSpace(space) => {
                self.handle_select_space(space);
            }
            UiCommand::SelectSubspace(subspace) => {
                self.handle_select_subspace(subspace);
            }
            UiCommand::MoveSpace { from, to } => {
                self.room_directory
                    .move_space(from, to, self.storage.as_ref())
                    .await;
            }
            UiCommand::SelectRoom(room_id) => {
                self.select_room(room_id).await;
            }
            UiCommand::RetryTimeline => {
                self.active_timeline.retry().await;
            }
            UiCommand::SendMessage {
                room_id,
                body,
                reply_to,
            } => {
                self.active_timeline
                    .spawn_send(&mut self.operations, room_id, body, reply_to);
            }
            UiCommand::PaginateBackwards { room_id } => {
                self.active_timeline.paginate_backwards(&room_id);
            }
            UiCommand::PaginateForwards { room_id } => {
                self.active_timeline.paginate_forwards(&room_id);
            }
            UiCommand::TimelinePaginationCompleted {
                room_id,
                direction,
                outcome,
            } => {
                self.active_timeline
                    .complete_pagination(&room_id, direction, outcome);
            }
            UiCommand::JumpToLatest { room_id } => {
                self.active_timeline.jump_to_latest(&room_id);
            }
            UiCommand::ScrollPositionChanged { at_top, at_bottom } => {
                self.active_timeline
                    .scroll_position_changed(at_top, at_bottom);
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

    fn log_command(cmd: &UiCommand) {
        if matches!(
            cmd,
            UiCommand::RoomsUpdated(_) | UiCommand::SpacesUpdated(_)
        ) {
            tracing::debug!(command = %cmd, "handling command");
        } else {
            tracing::info!(command = %cmd, "handling command");
        }
    }

    fn handle_rooms_updated(&mut self, rooms: Vec<Room>) {
        if self.room_directory.store_rooms(rooms) {
            self.refresh_selected_room();
            self.room_directory.emit_directory(&self.selection);
        }
    }

    fn handle_spaces_updated(&mut self, spaces: Vec<Space>) {
        if self.room_directory.store_spaces(spaces) {
            let outcome = self.room_directory.reconcile(&mut self.selection);
            if outcome.space_dropped {
                self.output.selected_space(String::new());
                self.output.selected_subspace(String::new());
            } else if outcome.subspace_dropped {
                self.output.selected_subspace(String::new());
            }
            self.room_directory.emit_directory(&self.selection);
        }
    }

    fn handle_select_space(&mut self, space: Option<RoomId>) {
        self.selection.set_space(space);
        self.output.selected_space(self.selection.space_id_str());
        self.output
            .selected_subspace(self.selection.subspace_id_str());
        self.room_directory.emit_subspaces(&self.selection);
        self.room_directory.emit_rooms(&self.selection);
    }

    fn handle_select_subspace(&mut self, subspace: Option<RoomId>) {
        self.selection.set_subspace(subspace);
        self.output
            .selected_subspace(self.selection.subspace_id_str());
        self.room_directory.emit_rooms(&self.selection);
    }

    async fn select_room(&mut self, room_id: RoomId) {
        self.selection.room = Some(room_id.clone());
        let (name, member_count) = self
            .room_directory
            .selected_room_meta(&self.selection)
            .map_or_else(|| (String::new(), 0), |m| (m.name, m.member_count));
        self.output
            .selected_room(room_id.clone(), name, member_count);
        self.active_timeline.select_room(room_id).await;
    }

    fn refresh_selected_room(&mut self) {
        let Some(room_id) = self.selection.room.clone() else {
            return;
        };
        if let Some(meta) = self.room_directory.selected_room_meta(&self.selection) {
            self.output
                .selected_room(room_id, meta.name, meta.member_count);
        } else {
            self.selection.room = None;
            self.output
                .selected_room(RoomId::new(String::new()), String::new(), 0);
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
        );
        self.session.spawn_user_avatar_fetch(&mut self.background);
    }

    async fn start_background_tasks(&mut self) {
        self.background.reset().await;
        self.session.spawn_session_persister(&mut self.background);
        self.verification.spawn_forwarder(&mut self.background);
    }

    async fn shutdown_all_tasks(&mut self) {
        self.background.shutdown().await;
        self.active_timeline.shutdown().await;
        self.operations.reset().await;
    }

    async fn handle_session_expired(&mut self) {
        tracing::info!("session expired, clearing local state");
        self.shutdown_all_tasks().await;
        self.room_directory.reset();
        self.selection = Selection::default();
        self.session.spawn_expire_session(&mut self.operations);
    }

    async fn handle_logout(&mut self) {
        self.shutdown_all_tasks().await;
        self.room_directory.reset();
        self.selection = Selection::default();
        self.session.spawn_logout(&mut self.operations);
    }

    async fn handle_quit(&mut self) {
        self.shutdown_all_tasks().await;
        self.media.drain().await;
    }
}
