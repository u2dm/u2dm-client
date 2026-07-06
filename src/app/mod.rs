mod active_timeline;
mod media;
mod room_directory;
mod session;
mod task_group;
mod verification;

use std::sync::Arc;

use active_timeline::ActiveTimeline;
use media::MediaActions;
use room_directory::RoomDirectory;
use session::SessionController;
use task_group::TaskGroup;
use tokio::sync::mpsc;
use verification::VerificationController;

use crate::commands::UiCommand;
use crate::domain::models::ConnectionStatus;
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
    session: SessionController,
    room_directory: RoomDirectory,
    active_timeline: ActiveTimeline,
    verification: VerificationController,
    media: MediaActions,
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
        }
    }

    pub async fn run(&mut self) {
        while let Some(cmd) = self.cmd_rx.recv().await {
            Self::log_command(&cmd);
            match cmd {
                UiCommand::RestoreSession => {
                    self.session.restore_session().await;
                }
                UiCommand::CheckServer(homeserver) => {
                    self.session.check_server(&homeserver).await;
                }
                UiCommand::LoginPassword(creds) => {
                    self.session.login_password(creds).await;
                }
                UiCommand::LoginOAuth => {
                    self.session.login_oauth().await;
                }
                UiCommand::FetchRooms => {
                    self.handle_fetch_rooms().await;
                }
                UiCommand::RoomsUpdated(rooms) => {
                    self.room_directory.update_rooms(rooms);
                }
                UiCommand::SpacesUpdated(spaces) => {
                    self.room_directory.update_spaces(spaces);
                }
                UiCommand::SelectSpace(space) => {
                    self.room_directory.select_space(space);
                }
                UiCommand::MoveSpace { from, to } => {
                    self.room_directory
                        .move_space(from, to, self.storage.as_ref())
                        .await;
                }
                UiCommand::SelectRoom(room_id) => {
                    self.active_timeline.select_room(room_id).await;
                }
                UiCommand::SendMessage {
                    room_id,
                    body,
                    reply_to,
                } => {
                    self.active_timeline
                        .send_message(room_id, body, reply_to)
                        .await;
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
                    hit_end,
                } => {
                    self.active_timeline
                        .complete_pagination(&room_id, direction, hit_end);
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
                    self.verification.accept().await;
                }
                UiCommand::RejectVerification => {
                    self.verification.reject().await;
                }
                UiCommand::ConfirmVerification => {
                    self.verification.confirm().await;
                }
                UiCommand::SessionExpired => {
                    self.handle_session_expired().await;
                }
                UiCommand::Logout => {
                    self.handle_logout().await;
                }
                UiCommand::Quit => {
                    self.handle_quit().await;
                    break;
                }
            }
        }
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
    }

    async fn handle_session_expired(&mut self) {
        tracing::info!("session expired, clearing local state");
        self.shutdown_all_tasks().await;
        self.room_directory.reset();
        self.session.expire_session().await;
    }

    async fn handle_logout(&mut self) {
        self.shutdown_all_tasks().await;
        self.room_directory.reset();
        self.session.logout().await;
    }

    async fn handle_quit(&mut self) {
        self.shutdown_all_tasks().await;
        self.media.drain().await;
    }
}
