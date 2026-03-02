use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::commands::{UiCommand, UiEvent};
use crate::domain::models::{
    ConnectionStatus, LoginCredentials, RoomId, Session, SyncSnapshot, TimelineMessage,
};
use crate::error::Result;
use crate::ports::matrix::MatrixPort;
use crate::ports::storage::StoragePort;

pub struct AppService {
    matrix: Arc<dyn MatrixPort>,
    storage: Arc<dyn StoragePort>,
    cmd_rx: mpsc::Receiver<UiCommand>,
    cmd_tx: mpsc::Sender<UiCommand>,
    ui_tx: mpsc::Sender<UiEvent>,
    timeline_handle: Option<JoinHandle<()>>,
    sync_handle: Option<(JoinHandle<()>, JoinHandle<()>)>,
}

impl AppService {
    pub fn new(
        matrix: Arc<dyn MatrixPort>,
        storage: Arc<dyn StoragePort>,
        cmd_rx: mpsc::Receiver<UiCommand>,
        cmd_tx: mpsc::Sender<UiCommand>,
        ui_tx: mpsc::Sender<UiEvent>,
    ) -> Self {
        Self {
            matrix,
            storage,
            cmd_rx,
            cmd_tx,
            ui_tx,
            timeline_handle: None,
            sync_handle: None,
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

    async fn emit(&self, event: UiEvent) {
        if let Err(e) = self.ui_tx.send(event).await {
            tracing::debug!("failed to send UI event: {e}");
        }
    }

    async fn send_cmd(&self, cmd: UiCommand) {
        if let Err(e) = self.cmd_tx.send(cmd).await {
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

    async fn handle_restore_session(&self) {
        let session = match self.storage.load_session().await {
            Ok(Some(session)) => session,
            Ok(None) => {
                if let Err(e) = self.matrix.clear_store().await {
                    tracing::warn!("failed to clear store on missing session: {e}");
                }
                return;
            }
            Err(e) => {
                self.emit(UiEvent::Error(e.to_string())).await;
                return;
            }
        };

        self.emit(UiEvent::Status("Restoring session...".into()))
            .await;

        match self.matrix.restore_session(&session).await {
            Ok(()) => {
                self.emit(UiEvent::LoginSuccess {
                    user_id: session.user_id,
                })
                .await;
                self.send_cmd(UiCommand::FetchRooms).await;
            }
            Err(e) => {
                self.clear_local_state().await;
                self.emit(UiEvent::Error(e.to_string())).await;
            }
        }
    }

    async fn handle_check_server(&self, homeserver: &str) {
        match self.matrix.discover_auth(homeserver).await {
            Ok(info) => self.emit(UiEvent::ServerInfo(info)).await,
            Err(e) => self.emit(UiEvent::Error(e.to_string())).await,
        }
    }

    async fn handle_login_password(&self, creds: LoginCredentials) {
        match self.matrix.login_password(creds).await {
            Ok(session) => {
                self.save_session(&session).await;
                self.emit(UiEvent::LoginSuccess {
                    user_id: session.user_id,
                })
                .await;
                self.send_cmd(UiCommand::FetchRooms).await;
            }
            Err(e) => self.emit(UiEvent::Error(e.to_string())).await,
        }
    }

    async fn handle_login_oauth(&self) {
        match self.run_oauth_flow().await {
            Ok(()) => {
                self.send_cmd(UiCommand::FetchRooms).await;
            }
            Err(e) => self.emit(UiEvent::Error(e.to_string())).await,
        }
    }

    async fn run_oauth_flow(&self) -> Result<()> {
        let oauth_data = self.matrix.login_oauth_start().await?;
        open::that_in_background(&oauth_data.auth_url);
        self.emit(UiEvent::Status("Waiting for authentication...".into()))
            .await;
        let session = self.matrix.login_oauth_finish().await?;
        self.save_session(&session).await;
        self.emit(UiEvent::LoginSuccess {
            user_id: session.user_id,
        })
        .await;
        Ok(())
    }

    async fn save_session(&self, session: &Session) {
        if let Err(e) = self.storage.save_session(session).await {
            tracing::warn!("failed to save session: {e}");
        }
    }

    fn abort_sync(&mut self) {
        if let Some((sync_task, receiver_task)) = self.sync_handle.take() {
            sync_task.abort();
            receiver_task.abort();
        }
    }

    fn handle_quit(&mut self) {
        if let Some(handle) = self.timeline_handle.take() {
            handle.abort();
        }
        self.abort_sync();
    }

    async fn handle_session_expired(&mut self) {
        tracing::info!("session expired, clearing local state");
        if let Some(handle) = self.timeline_handle.take() {
            handle.abort();
        }
        self.abort_sync();
        self.clear_local_state().await;
        self.emit(UiEvent::LoggedOut).await;
        self.emit(UiEvent::Error(
            "Session expired. Please log in again.".into(),
        ))
        .await;
    }

    async fn handle_logout(&mut self) {
        if let Some(handle) = self.timeline_handle.take() {
            handle.abort();
        }
        self.abort_sync();
        if let Err(e) = self.matrix.logout().await {
            tracing::warn!("failed to logout from server: {e}");
        }
        self.clear_local_state().await;
        self.emit(UiEvent::ConnectionStatus(ConnectionStatus::Disconnected))
            .await;
        self.emit(UiEvent::LoggedOut).await;
    }

    fn handle_select_room(&mut self, room_id: RoomId) {
        if let Some(handle) = self.timeline_handle.take() {
            handle.abort();
        }
        self.timeline_handle = Some(Self::spawn_timeline_subscription(
            &self.matrix,
            &self.ui_tx,
            room_id,
        ));
    }

    fn handle_send_message(&self, room_id: RoomId, body: String) {
        let matrix = Arc::clone(&self.matrix);
        let ui_tx = self.ui_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = matrix.send_text(&room_id, &body).await {
                tracing::warn!("failed to send message: {e}");
                if let Err(send_err) = ui_tx
                    .send(UiEvent::Error(format!("Failed to send message: {e}")))
                    .await
                {
                    tracing::debug!("failed to send Error event: {send_err}");
                }
            }
        });
    }

    async fn handle_fetch_rooms(&mut self) {
        self.abort_sync();

        self.emit(UiEvent::ConnectionStatus(ConnectionStatus::Connecting))
            .await;

        match self.matrix.rooms().await {
            Ok(rooms) => {
                self.emit(UiEvent::Rooms(rooms)).await;
                self.emit(UiEvent::ConnectionStatus(ConnectionStatus::Connected))
                    .await;
            }
            Err(e) => {
                self.emit(UiEvent::Error(e.to_string())).await;
                self.emit(UiEvent::ConnectionStatus(ConnectionStatus::Error(
                    e.to_string(),
                )))
                .await;
            }
        }

        let (snapshot_tx, mut snapshot_rx) = mpsc::channel::<SyncSnapshot>(16);
        let matrix_sync = Arc::clone(&self.matrix);
        let sync_task = tokio::spawn(async move {
            if let Err(e) = matrix_sync.start_sync(snapshot_tx).await {
                tracing::error!("sync loop ended with error: {e}");
            }
        });

        let ui_tx = self.ui_tx.clone();
        let cmd_tx = self.cmd_tx.clone();
        let receiver_task = tokio::spawn(async move {
            while let Some(snapshot) = snapshot_rx.recv().await {
                if let Err(e) = ui_tx
                    .send(UiEvent::ConnectionStatus(
                        snapshot.connection_status.clone(),
                    ))
                    .await
                {
                    tracing::debug!("failed to send ConnectionStatus event: {e}");
                }
                if matches!(snapshot.connection_status, ConnectionStatus::Connected)
                    && let Err(e) = ui_tx.send(UiEvent::Rooms(snapshot.rooms)).await
                {
                    tracing::debug!("failed to send Rooms event: {e}");
                }
            }

            if let Err(e) = cmd_tx.send(UiCommand::SessionExpired).await {
                tracing::debug!("failed to send SessionExpired command: {e}");
            }
        });

        self.sync_handle = Some((sync_task, receiver_task));
    }

    fn spawn_timeline_subscription(
        matrix: &Arc<dyn MatrixPort>,
        ui_tx: &mpsc::Sender<UiEvent>,
        room_id: RoomId,
    ) -> JoinHandle<()> {
        let (tl_tx, mut tl_rx) = mpsc::channel::<Vec<TimelineMessage>>(16);
        let matrix_tl = Arc::clone(matrix);
        let ui_tx = ui_tx.clone();

        tokio::spawn(async move {
            let subscribe = matrix_tl.subscribe_timeline(&room_id, tl_tx);
            let forward = async {
                while let Some(messages) = tl_rx.recv().await {
                    if let Err(e) = ui_tx.send(UiEvent::Timeline(messages)).await {
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
            }
        })
    }
}
