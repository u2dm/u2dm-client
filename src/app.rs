use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::commands::{UiCommand, UiEvent};
use crate::domain::models::{
    ConnectionStatus, LoginCredentials, RoomId, Session, SyncSnapshot, TimelineMessage,
    VerificationEvent,
};

use crate::error::{AppError, Result};
use crate::ports::matrix::MatrixPort;
use crate::ports::storage::StoragePort;

pub struct AppService {
    matrix: Arc<dyn MatrixPort>,
    storage: Arc<dyn StoragePort>,
    cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
    background_token: CancellationToken,
    timeline_token: CancellationToken,
}

impl AppService {
    pub fn new(
        matrix: Arc<dyn MatrixPort>,
        storage: Arc<dyn StoragePort>,
        cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
        cmd_tx: mpsc::UnboundedSender<UiCommand>,
        ui_tx: mpsc::UnboundedSender<UiEvent>,
    ) -> Self {
        Self {
            matrix,
            storage,
            cmd_rx,
            cmd_tx,
            ui_tx,
            background_token: CancellationToken::new(),
            timeline_token: CancellationToken::new(),
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
                    self.handle_select_room(room_id);
                }
                UiCommand::SendMessage { room_id, body } => {
                    self.handle_send_message(room_id, body);
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
                    self.handle_quit();
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

    fn send_cmd(&self, cmd: UiCommand) {
        if let Err(e) = self.cmd_tx.send(cmd) {
            tracing::debug!("failed to send command: {e}");
        }
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
                self.emit(UiEvent::Error(e.to_string()));
                return;
            }
        };

        self.emit(UiEvent::Status("Restoring session...".into()));

        match self.matrix.restore_session(&session).await {
            Ok(()) => {
                self.emit(UiEvent::LoginSuccess {
                    user_id: session.user_id,
                });
                self.send_cmd(UiCommand::FetchRooms);
            }
            Err(e) => {
                self.clear_local_state().await;
                self.emit(UiEvent::Error(e.to_string()));
            }
        }
    }

    async fn handle_check_server(&mut self, homeserver: &str) {
        match self.matrix.discover_auth(homeserver).await {
            Ok(info) => self.emit(UiEvent::ServerInfo(info)),
            Err(e) => self.emit(UiEvent::Error(e.to_string())),
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
            Err(e) => self.emit(UiEvent::Error(e.to_string())),
        }
    }

    async fn handle_login_oauth(&mut self) {
        match self.run_oauth_flow().await {
            Ok(()) => {
                self.send_cmd(UiCommand::FetchRooms);
            }
            Err(e) => self.emit(UiEvent::Error(e.to_string())),
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

    fn shutdown_all_tasks(&mut self) {
        self.background_token.cancel();
        self.background_token = CancellationToken::new();
        self.timeline_token.cancel();
        self.timeline_token = CancellationToken::new();
    }

    fn handle_quit(&mut self) {
        self.shutdown_all_tasks();
    }

    async fn handle_session_expired(&mut self) {
        tracing::info!("session expired, clearing local state");
        self.shutdown_all_tasks();
        self.clear_local_state().await;
        self.emit(UiEvent::LoggedOut);
        self.emit(UiEvent::Error(
            "Session expired. Please log in again.".into(),
        ));
    }

    async fn handle_logout(&mut self) {
        self.shutdown_all_tasks();
        if let Err(e) = self.matrix.logout().await {
            tracing::warn!("failed to logout from server: {e}");
        }
        self.clear_local_state().await;
        self.emit(UiEvent::ConnectionStatus(ConnectionStatus::Disconnected));
        self.emit(UiEvent::LoggedOut);
    }

    fn handle_select_room(&mut self, room_id: RoomId) {
        self.timeline_token.cancel();
        self.timeline_token = CancellationToken::new();
        Self::spawn_timeline_subscription(
            &self.matrix,
            &self.ui_tx,
            room_id,
            self.timeline_token.clone(),
        );
    }

    fn handle_send_message(&mut self, room_id: RoomId, body: String) {
        let matrix = Arc::clone(&self.matrix);
        let ui_tx = self.ui_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = matrix.send_text(&room_id, &body).await {
                tracing::warn!("failed to send message: {e}");
                if let Err(send_err) =
                    ui_tx.send(UiEvent::Error(format!("Failed to send message: {e}")))
                {
                    tracing::debug!("failed to send Error event: {send_err}");
                }
            }
        });
    }

    async fn handle_accept_verification(&mut self) {
        if let Err(e) = self.matrix.accept_verification().await {
            self.emit(UiEvent::Error(format!("Verification accept failed: {e}")));
        }
    }

    async fn handle_reject_verification(&mut self) {
        if let Err(e) = self.matrix.reject_verification().await {
            self.emit(UiEvent::Error(format!("Verification reject failed: {e}")));
        }
    }

    async fn handle_confirm_verification(&mut self) {
        if let Err(e) = self.matrix.confirm_verification().await {
            self.emit(UiEvent::Error(format!("Verification confirm failed: {e}")));
        }
    }

    async fn handle_fetch_rooms(&mut self) {
        self.start_background_listeners();

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
                self.emit(UiEvent::Error(e.to_string()));
                self.emit(UiEvent::ConnectionStatus(ConnectionStatus::Error(
                    e.to_string(),
                )));
            }
        }

        self.start_sync_pipeline();
    }

    fn start_background_listeners(&mut self) {
        self.background_token.cancel();
        self.background_token = CancellationToken::new();

        Self::spawn_session_change_listener(
            &self.matrix,
            &self.storage,
            self.background_token.clone(),
        );

        Self::spawn_verification_listener(&self.matrix, &self.ui_tx, self.background_token.clone());
    }

    fn start_sync_pipeline(&mut self) {
        let (snapshot_tx, mut snapshot_rx) = mpsc::unbounded_channel::<SyncSnapshot>();
        let matrix_sync = Arc::clone(&self.matrix);
        let token = self.background_token.clone();
        tokio::spawn(async move {
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
        let token = self.background_token.clone();
        tokio::spawn(async move {
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

            if let Err(e) = cmd_tx.send(UiCommand::SessionExpired) {
                tracing::debug!("failed to send SessionExpired command: {e}");
            }
        });
    }

    fn spawn_verification_listener(
        matrix: &Arc<dyn MatrixPort>,
        ui_tx: &mpsc::UnboundedSender<UiEvent>,
        token: CancellationToken,
    ) {
        let (verif_tx, mut verif_rx) = mpsc::unbounded_channel::<VerificationEvent>();
        let matrix_verif = Arc::clone(matrix);
        let ui_tx = ui_tx.clone();

        tokio::spawn(async move {
            let listen = matrix_verif.listen_for_verification(verif_tx);
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

    fn spawn_session_change_listener(
        matrix: &Arc<dyn MatrixPort>,
        storage: &Arc<dyn StoragePort>,
        token: CancellationToken,
    ) {
        let (session_tx, mut session_rx) = mpsc::unbounded_channel::<Session>();
        let matrix = Arc::clone(matrix);
        let storage = Arc::clone(storage);

        tokio::spawn(async move {
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

    fn spawn_timeline_subscription(
        matrix: &Arc<dyn MatrixPort>,
        ui_tx: &mpsc::UnboundedSender<UiEvent>,
        room_id: RoomId,
        token: CancellationToken,
    ) {
        let (tl_tx, mut tl_rx) = mpsc::unbounded_channel::<Vec<TimelineMessage>>();
        let matrix_tl = Arc::clone(matrix);
        let ui_tx = ui_tx.clone();

        tokio::spawn(async move {
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
}
