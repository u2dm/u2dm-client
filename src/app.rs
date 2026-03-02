use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::commands::{UiCommand, UiEvent};
use crate::domain::models::{LoginCredentials, RoomId, Session, SyncSnapshot, TimelineMessage};
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
                UiCommand::Logout => {
                    self.handle_logout().await;
                }
            }
        }
    }

    async fn emit(&self, event: UiEvent) {
        drop(self.ui_tx.send(event).await);
    }

    async fn handle_restore_session(&self) {
        let session = match self.storage.load_session().await {
            Ok(Some(session)) => session,
            Ok(None) => return,
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
                drop(self.cmd_tx.send(UiCommand::FetchRooms).await);
            }
            Err(e) => {
                // session is stale, clear it and let user log in again
                drop(self.storage.clear_session().await);
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
                drop(self.cmd_tx.send(UiCommand::FetchRooms).await);
            }
            Err(e) => self.emit(UiEvent::Error(e.to_string())).await,
        }
    }

    async fn handle_login_oauth(&self) {
        match self.run_oauth_flow().await {
            Ok(()) => {
                drop(self.cmd_tx.send(UiCommand::FetchRooms).await);
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
        drop(self.storage.save_session(session).await);
    }

    async fn handle_logout(&mut self) {
        if let Some(handle) = self.timeline_handle.take() {
            handle.abort();
        }
        drop(self.matrix.logout().await);
        drop(self.storage.clear_session().await);
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
        tokio::spawn(async move {
            let _r = matrix.send_text(&room_id, &body).await;
        });
    }

    async fn handle_fetch_rooms(&self) {
        match self.matrix.rooms().await {
            Ok(rooms) => self.emit(UiEvent::Rooms(rooms)).await,
            Err(e) => self.emit(UiEvent::Error(e.to_string())).await,
        }

        let (snapshot_tx, mut snapshot_rx) = mpsc::channel::<SyncSnapshot>(16);
        let matrix_sync = Arc::clone(&self.matrix);
        tokio::spawn(async move {
            let _r = matrix_sync.start_sync(snapshot_tx).await;
        });

        let ui_tx = self.ui_tx.clone();
        tokio::spawn(async move {
            while let Some(snapshot) = snapshot_rx.recv().await {
                drop(ui_tx.send(UiEvent::Rooms(snapshot.rooms)).await);
            }
        });
    }

    fn spawn_timeline_subscription(
        matrix: &Arc<dyn MatrixPort>,
        ui_tx: &mpsc::Sender<UiEvent>,
        room_id: RoomId,
    ) -> JoinHandle<()> {
        let (tl_tx, mut tl_rx) = mpsc::channel::<Vec<TimelineMessage>>(16);
        let matrix_tl = Arc::clone(matrix);

        tokio::spawn(async move {
            let _r = matrix_tl.subscribe_timeline(&room_id, tl_tx).await;
        });

        let ui_tx = ui_tx.clone();
        tokio::spawn(async move {
            while let Some(messages) = tl_rx.recv().await {
                drop(ui_tx.send(UiEvent::Timeline(messages)).await);
            }
        })
    }
}
