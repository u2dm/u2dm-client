use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use rand::RngExt;
use rand::distr::Alphanumeric;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::{fs, time};
use tokio_util::sync::CancellationToken;

use crate::commands::{UiCommand, UiEvent};
use crate::config::AppConfig;
use crate::domain::models::{
    ConnectionStatus, LoginCredentials, RoomId, Session, SyncSnapshot, TimelineMessage,
    UiErrorKind, VerificationEvent,
};
use crate::error::{AppError, Result};
use crate::ports::matrix::MatrixPort;
use crate::ports::storage::StoragePort;

fn generate_passphrase() -> String {
    (&mut rand::rng())
        .sample_iter(Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

struct TaskGroup {
    token: CancellationToken,
    tasks: JoinSet<()>,
}

impl TaskGroup {
    fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            tasks: JoinSet::new(),
        }
    }

    async fn reset(&mut self) {
        self.token.cancel();
        self.tasks.abort_all();
        while self.tasks.join_next().await.is_some() {}
        self.token = CancellationToken::new();
    }

    async fn shutdown(&mut self) {
        self.token.cancel();
        self.tasks.abort_all();
        while self.tasks.join_next().await.is_some() {}
    }

    fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    fn spawn(&mut self, future: impl Future<Output = ()> + Send + 'static) {
        self.tasks.spawn(future);
    }
}

pub struct AppService {
    matrix: Arc<dyn MatrixPort>,
    storage: Arc<dyn StoragePort>,
    config: AppConfig,
    cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
    background: TaskGroup,
    timeline: TaskGroup,
    fire_and_forget: JoinSet<()>,
}

impl AppService {
    pub fn new(
        matrix: Arc<dyn MatrixPort>,
        storage: Arc<dyn StoragePort>,
        config: AppConfig,
        cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
        cmd_tx: mpsc::UnboundedSender<UiCommand>,
        ui_tx: mpsc::UnboundedSender<UiEvent>,
    ) -> Self {
        Self {
            matrix,
            storage,
            config,
            cmd_rx,
            cmd_tx,
            ui_tx,
            background: TaskGroup::new(),
            timeline: TaskGroup::new(),
            fire_and_forget: JoinSet::new(),
        }
    }

    pub async fn run(&mut self) {
        while let Some(cmd) = self.cmd_rx.recv().await {
            match cmd {
                UiCommand::RestoreSession => {
                    self.handle_restore_session().await;
                }
                UiCommand::CheckServer(homeserver) => {
                    self.handle_check_server(&homeserver).await;
                }
                UiCommand::LoginPassword(creds) => {
                    self.handle_login_password(creds).await;
                }
                UiCommand::LoginOAuth(_homeserver) => {
                    self.handle_login_oauth().await;
                }
                UiCommand::FetchRooms => {
                    self.handle_fetch_rooms().await;
                }
                UiCommand::SelectRoom(room_id) => {
                    self.handle_select_room(room_id).await;
                }
                UiCommand::SendMessage { room_id, body } => {
                    self.handle_send_message(room_id, body);
                }
                UiCommand::OpenMedia { event_id } => {
                    self.handle_open_media(event_id);
                }
                UiCommand::SaveFile { event_id, filename } => {
                    self.handle_save_file(event_id, filename);
                }
                UiCommand::AcceptVerification => {
                    self.handle_accept_verification().await;
                }
                UiCommand::RejectVerification => {
                    self.handle_reject_verification().await;
                }
                UiCommand::ConfirmVerification => {
                    self.handle_confirm_verification().await;
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

    fn emit(&self, event: UiEvent) {
        if let Err(e) = self.ui_tx.send(event) {
            tracing::debug!("failed to send UI event: {e}");
        }
    }

    fn emit_error(&self, err: &AppError) {
        let kind = err.ui_error_kind();
        self.emit(UiEvent::Error {
            message: err.to_string(),
            kind,
        });
    }

    fn send_cmd(&self, cmd: UiCommand) {
        if let Err(e) = self.cmd_tx.send(cmd) {
            tracing::debug!("failed to send command: {e}");
        }
    }

    async fn get_or_create_passphrase(&self) -> Result<String> {
        if let Some(passphrase) = self.storage.load_passphrase().await? {
            return Ok(passphrase);
        }
        let passphrase = generate_passphrase();
        self.storage.save_passphrase(&passphrase).await?;
        Ok(passphrase)
    }

    async fn clear_local_state(&self) {
        if let Err(e) = self.storage.clear_session().await {
            tracing::warn!("failed to clear session: {e}");
        }
        if let Err(e) = self.matrix.clear_store().await {
            tracing::warn!("failed to clear store: {e}");
        }
    }

    async fn handle_restore_session(&mut self) {
        let session = match self.storage.load_session().await {
            Ok(Some(session)) => session,
            Ok(None) => {
                if let Err(e) = self.matrix.clear_store().await {
                    tracing::warn!("failed to clear store on missing session: {e}");
                }
                return;
            }
            Err(e) => {
                self.emit_error(&e);
                return;
            }
        };

        self.emit(UiEvent::Status("Restoring session...".into()));

        let passphrase = match self.get_or_create_passphrase().await {
            Ok(p) => p,
            Err(e) => {
                self.emit_error(&e);
                return;
            }
        };

        match self.matrix.restore_session(&session, &passphrase).await {
            Ok(()) => {
                self.emit(UiEvent::LoginSuccess {
                    user_id: session.user_id,
                });
                self.send_cmd(UiCommand::FetchRooms);
            }
            Err(e) => {
                self.clear_local_state().await;
                self.emit_error(&e);
            }
        }
    }

    async fn handle_check_server(&mut self, homeserver: &str) {
        let passphrase = match self.get_or_create_passphrase().await {
            Ok(p) => p,
            Err(e) => {
                self.emit_error(&e);
                return;
            }
        };
        match self.matrix.discover_auth(homeserver, &passphrase).await {
            Ok(info) => self.emit(UiEvent::ServerInfo(info)),
            Err(e) => self.emit_error(&e),
        }
    }

    async fn handle_login_password(&mut self, creds: LoginCredentials) {
        match self.matrix.login_password(creds).await {
            Ok(session) => {
                self.save_session(&session).await;
                self.emit(UiEvent::LoginSuccess {
                    user_id: session.user_id,
                });
                self.send_cmd(UiCommand::FetchRooms);
            }
            Err(e) => self.emit_error(&e),
        }
    }

    async fn handle_login_oauth(&mut self) {
        match self.run_oauth_flow().await {
            Ok(()) => {
                self.send_cmd(UiCommand::FetchRooms);
            }
            Err(e) => self.emit_error(&e),
        }
    }

    async fn run_oauth_flow(&mut self) -> Result<()> {
        let oauth_data = self.matrix.login_oauth_start().await?;
        open::that_in_background(&oauth_data.auth_url);
        self.emit(UiEvent::Status("Waiting for authentication...".into()));
        let session = self.matrix.login_oauth_finish().await?;
        self.save_session(&session).await;
        self.emit(UiEvent::LoginSuccess {
            user_id: session.user_id,
        });
        Ok(())
    }

    async fn save_session(&self, session: &Session) {
        if let Err(e) = self.storage.save_session(session).await {
            tracing::warn!("failed to save session: {e}");
        }
    }

    async fn shutdown_all_tasks(&mut self) {
        self.background.shutdown().await;
        self.timeline.shutdown().await;
    }

    async fn handle_quit(&mut self) {
        self.shutdown_all_tasks().await;
        self.drain_fire_and_forget().await;
    }

    async fn drain_fire_and_forget(&mut self) {
        if self.fire_and_forget.is_empty() {
            return;
        }
        let count = self.fire_and_forget.len();
        tracing::debug!("waiting for {count} in-flight task(s)");
        let result = time::timeout(Duration::from_secs(3), async {
            while self.fire_and_forget.join_next().await.is_some() {}
        })
        .await;
        if result.is_err() {
            tracing::warn!("timed out waiting for in-flight tasks, abandoning");
            self.fire_and_forget.abort_all();
        }
    }

    async fn handle_session_expired(&mut self) {
        tracing::info!("session expired, clearing local state");
        self.shutdown_all_tasks().await;
        self.clear_local_state().await;
        self.emit(UiEvent::LoggedOut);
        self.emit(UiEvent::Error {
            message: "Session expired. Please log in again.".into(),
            kind: UiErrorKind::Authentication,
        });
    }

    async fn handle_logout(&mut self) {
        self.shutdown_all_tasks().await;
        if let Err(e) = self.matrix.logout().await {
            tracing::warn!("failed to logout from server: {e}");
        }
        self.clear_local_state().await;
        self.emit(UiEvent::ConnectionStatus(ConnectionStatus::Disconnected));
        self.emit(UiEvent::LoggedOut);
    }

    async fn handle_select_room(&mut self, room_id: RoomId) {
        self.timeline.reset().await;

        let (tl_tx, mut tl_rx) = mpsc::unbounded_channel::<Vec<TimelineMessage>>();
        let matrix_tl = Arc::clone(&self.matrix);
        let ui_tx = self.ui_tx.clone();
        let token = self.timeline.token();

        self.timeline.spawn(async move {
            let subscribe = matrix_tl.subscribe_timeline(&room_id, tl_tx);
            let forward = async {
                while let Some(messages) = tl_rx.recv().await {
                    if let Err(e) = ui_tx.send(UiEvent::Timeline(messages)) {
                        tracing::debug!("failed to send Timeline event: {e}");
                        break;
                    }
                }
            };

            tokio::select! {
                result = subscribe => {
                    if let Err(e) = result {
                        tracing::warn!("timeline subscription failed: {e}");
                    }
                }
                () = forward => {
                    tracing::debug!("timeline forwarder stopped");
                }
                () = token.cancelled() => {
                    tracing::debug!("timeline subscription cancelled");
                }
            }
        });
    }

    fn reap_finished(&mut self) {
        while self.fire_and_forget.try_join_next().is_some() {}
    }

    fn handle_send_message(&mut self, room_id: RoomId, body: String) {
        self.reap_finished();

        let matrix = Arc::clone(&self.matrix);
        let ui_tx = self.ui_tx.clone();
        self.fire_and_forget.spawn(async move {
            if let Err(e) = matrix.send_text(&room_id, &body).await {
                tracing::warn!("failed to send message: {e}");
                if let Err(send_err) = ui_tx.send(UiEvent::Error {
                    message: format!("Failed to send message: {e}"),
                    kind: UiErrorKind::Other,
                }) {
                    tracing::debug!("failed to send Error event: {send_err}");
                }
            }
        });
    }

    fn handle_open_media(&mut self, event_id: String) {
        self.reap_finished();

        let matrix = Arc::clone(&self.matrix);
        let ui_tx = self.ui_tx.clone();
        let cache_dir = self.config.data_dir.join("media-cache");
        self.fire_and_forget.spawn(async move {
            match matrix.download_media(&event_id, false).await {
                Ok(data) => {
                    let cache_path = cache_dir.join(event_id.replace(':', "_"));
                    if let Err(e) = fs::create_dir_all(&cache_dir).await {
                        tracing::warn!("failed to create media cache dir: {e}");
                        return;
                    }
                    if let Err(e) = fs::write(&cache_path, &data).await {
                        tracing::warn!("failed to write media file: {e}");
                        return;
                    }
                    open::that_in_background(&cache_path);
                }
                Err(e) => {
                    ui_tx
                        .send(UiEvent::Error {
                            message: format!("Failed to download media: {e}"),
                            kind: UiErrorKind::Other,
                        })
                        .ok();
                }
            }
        });
    }

    fn handle_save_file(&mut self, event_id: String, filename: String) {
        self.reap_finished();

        let matrix = Arc::clone(&self.matrix);
        let ui_tx = self.ui_tx.clone();
        self.fire_and_forget.spawn(async move {
            let dialog = rfd::AsyncFileDialog::new().set_file_name(&filename);
            let Some(file_handle) = dialog.save_file().await else {
                return;
            };
            match matrix.download_media(&event_id, false).await {
                Ok(data) => {
                    if let Err(e) = file_handle.write(&data).await {
                        ui_tx
                            .send(UiEvent::Error {
                                message: format!("Failed to save file: {e}"),
                                kind: UiErrorKind::Other,
                            })
                            .ok();
                    } else {
                        ui_tx
                            .send(UiEvent::FileSaved {
                                path: file_handle.path().display().to_string(),
                            })
                            .ok();
                    }
                }
                Err(e) => {
                    ui_tx
                        .send(UiEvent::Error {
                            message: format!("Failed to download file: {e}"),
                            kind: UiErrorKind::Other,
                        })
                        .ok();
                }
            }
        });
    }

    async fn handle_accept_verification(&mut self) {
        if let Err(e) = self.matrix.accept_verification().await {
            self.emit(UiEvent::Error {
                message: format!("Verification accept failed: {e}"),
                kind: UiErrorKind::Other,
            });
        }
    }

    async fn handle_reject_verification(&mut self) {
        if let Err(e) = self.matrix.reject_verification().await {
            self.emit(UiEvent::Error {
                message: format!("Verification reject failed: {e}"),
                kind: UiErrorKind::Other,
            });
        }
    }

    async fn handle_confirm_verification(&mut self) {
        if let Err(e) = self.matrix.confirm_verification().await {
            self.emit(UiEvent::Error {
                message: format!("Verification confirm failed: {e}"),
                kind: UiErrorKind::Other,
            });
        }
    }

    async fn handle_fetch_rooms(&mut self) {
        self.start_background_listeners().await;

        self.emit(UiEvent::ConnectionStatus(ConnectionStatus::Connecting));

        match self.matrix.rooms().await {
            Ok(rooms) => {
                self.emit(UiEvent::Rooms(rooms));
                self.emit(UiEvent::ConnectionStatus(ConnectionStatus::Connected));
            }
            Err(AppError::SessionExpired) => {
                self.handle_session_expired().await;
                return;
            }
            Err(e) => {
                self.emit(UiEvent::ConnectionStatus(ConnectionStatus::Error(
                    e.to_string(),
                )));
            }
        }

        self.start_sync_pipeline();
    }

    async fn start_background_listeners(&mut self) {
        self.background.reset().await;
        Self::spawn_session_persister(&mut self.background, &self.matrix, &self.storage);
        Self::spawn_verification_forwarder(&mut self.background, &self.matrix, &self.ui_tx);
    }

    fn spawn_session_persister(
        group: &mut TaskGroup,
        matrix: &Arc<dyn MatrixPort>,
        storage: &Arc<dyn StoragePort>,
    ) {
        let matrix = Arc::clone(matrix);
        let storage = Arc::clone(storage);
        let token = group.token();
        group.spawn(async move {
            let (session_tx, mut session_rx) = mpsc::unbounded_channel::<Session>();
            let subscribe = matrix.subscribe_session_changes(session_tx);
            let persist = async {
                while let Some(session) = session_rx.recv().await {
                    if let Err(e) = storage.save_session(&session).await {
                        tracing::warn!("failed to persist refreshed session: {e}");
                    } else {
                        tracing::info!("persisted refreshed session tokens");
                    }
                }
            };

            tokio::select! {
                result = subscribe => {
                    if let Err(e) = result {
                        tracing::warn!("session change listener ended: {e}");
                    }
                }
                () = persist => {
                    tracing::debug!("session change persister stopped");
                }
                () = token.cancelled() => {
                    tracing::debug!("session change listener cancelled");
                }
            }
        });
    }

    fn spawn_verification_forwarder(
        group: &mut TaskGroup,
        matrix: &Arc<dyn MatrixPort>,
        ui_tx: &mpsc::UnboundedSender<UiEvent>,
    ) {
        let matrix = Arc::clone(matrix);
        let ui_tx = ui_tx.clone();
        let token = group.token();
        group.spawn(async move {
            let (verif_tx, mut verif_rx) = mpsc::unbounded_channel::<VerificationEvent>();
            let listen = matrix.listen_for_verification(verif_tx);
            let forward = async {
                while let Some(event) = verif_rx.recv().await {
                    if let Err(e) = ui_tx.send(UiEvent::Verification(event)) {
                        tracing::debug!("failed to send Verification event: {e}");
                        break;
                    }
                }
            };

            tokio::select! {
                result = listen => {
                    if let Err(e) = result {
                        tracing::warn!("verification listener ended: {e}");
                    }
                }
                () = forward => {
                    tracing::debug!("verification forwarder stopped");
                }
                () = token.cancelled() => {
                    tracing::debug!("verification listener cancelled");
                }
            }
        });
    }

    fn start_sync_pipeline(&mut self) {
        let (snapshot_tx, mut snapshot_rx) = mpsc::unbounded_channel::<SyncSnapshot>();

        let matrix_sync = Arc::clone(&self.matrix);
        let token = self.background.token();
        self.background.spawn(async move {
            tokio::select! {
                result = matrix_sync.start_sync(snapshot_tx) => {
                    if let Err(e) = result {
                        tracing::error!("sync loop ended with error: {e}");
                    }
                }
                () = token.cancelled() => {
                    tracing::debug!("sync task cancelled");
                }
            }
        });

        let ui_tx = self.ui_tx.clone();
        let cmd_tx = self.cmd_tx.clone();
        let token = self.background.token();
        self.background.spawn(async move {
            loop {
                tokio::select! {
                    snapshot = snapshot_rx.recv() => {
                        let Some(snapshot) = snapshot else { break };
                        if let Err(e) = ui_tx.send(UiEvent::ConnectionStatus(
                            snapshot.connection_status.clone(),
                        )) {
                            tracing::debug!("failed to send ConnectionStatus event: {e}");
                        }
                        if matches!(snapshot.connection_status, ConnectionStatus::Connected)
                            && let Err(e) = ui_tx.send(UiEvent::Rooms(snapshot.rooms))
                        {
                            tracing::debug!("failed to send Rooms event: {e}");
                        }
                    }
                    () = token.cancelled() => {
                        tracing::debug!("sync receiver cancelled");
                        return;
                    }
                }
            }

            if !token.is_cancelled()
                && let Err(e) = cmd_tx.send(UiCommand::SessionExpired)
            {
                tracing::debug!("failed to send SessionExpired command: {e}");
            }
        });
    }
}
