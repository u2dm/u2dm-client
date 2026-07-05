use std::collections::{HashMap, HashSet};
use std::env;
use std::fmt::Write;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::{fs, time};
use tokio_util::sync::CancellationToken;

use crate::commands::{UiCommand, UiEvent};
use crate::domain::models::{
    ConnectionStatus, LoginCredentials, Room, RoomId, ScrollMode, Session, Space, SyncEvent,
    TimelineCommand, TimelinePatch, VerificationEvent,
};
use crate::domain::viewport::ViewportController;
use crate::error::{AppError, Result};
use crate::ports::matrix::MatrixPort;
use crate::ports::storage::StoragePort;
use crate::util::hex_encode_id;

#[allow(clippy::let_underscore_must_use)]
fn generate_passphrase() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
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
    cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
    background: TaskGroup,
    timeline: TaskGroup,
    fire_and_forget: JoinSet<()>,
    viewport: ViewportController,
    timeline_cmd_tx: Option<mpsc::UnboundedSender<TimelineCommand>>,
    active_room_id: Option<RoomId>,
    at_bottom: Arc<AtomicBool>,
    new_messages_counter: Arc<AtomicU32>,
    all_rooms: Vec<Room>,
    spaces: Vec<Space>,
    space_children: HashMap<String, HashSet<String>>,
    selected_space: Option<RoomId>,
    connected: bool,
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
            background: TaskGroup::new(),
            timeline: TaskGroup::new(),
            fire_and_forget: JoinSet::new(),
            viewport: ViewportController::new(),
            timeline_cmd_tx: None,
            active_room_id: None,
            at_bottom: Arc::new(AtomicBool::new(true)),
            new_messages_counter: Arc::new(AtomicU32::new(0)),
            all_rooms: Vec::new(),
            spaces: Vec::new(),
            space_children: HashMap::new(),
            selected_space: None,
            connected: false,
        }
    }

    pub async fn run(&mut self) {
        while let Some(cmd) = self.cmd_rx.recv().await {
            Self::log_command(&cmd);
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
                UiCommand::LoginOAuth => {
                    self.handle_login_oauth().await;
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
                UiCommand::SelectRoom(room_id) => {
                    self.handle_select_room(room_id).await;
                }
                UiCommand::SendMessage { room_id, body } => {
                    self.handle_send_message(room_id, body).await;
                }
                UiCommand::PaginateBackwards { room_id } => {
                    self.handle_paginate_backwards(&room_id);
                }
                UiCommand::PaginateForwards { room_id } => {
                    self.handle_paginate_forwards(&room_id);
                }
                UiCommand::JumpToLatest { room_id } => {
                    self.handle_jump_to_latest(&room_id);
                }
                UiCommand::ScrollPositionChanged { at_top, at_bottom } => {
                    self.handle_scroll_position_changed(at_top, at_bottom);
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

    fn emit(&self, event: UiEvent) {
        if let Err(e) = self.ui_tx.send(event) {
            tracing::debug!("failed to send UI event: {e}");
        }
    }

    fn emit_show_login(&self) {
        self.emit(UiEvent::ShowLogin);
    }

    fn emit_login_error(&self, err: &AppError) {
        self.emit(UiEvent::LoginError(err.to_string()));
    }

    fn emit_toast_error(&self, msg: impl Into<String>) {
        self.emit(UiEvent::ToastError(msg.into()));
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

    #[allow(clippy::cognitive_complexity)]
    async fn handle_restore_session(&mut self) {
        self.emit(UiEvent::Status("loading-session".into()));

        let session = match self.storage.load_session().await {
            Ok(Some(session)) => {
                tracing::info!(user_id = %session.user_id, "found saved session");
                session
            }
            Ok(None) => {
                tracing::info!("no saved session found, showing login");
                if let Err(e) = self.matrix.clear_store().await {
                    tracing::warn!("failed to clear store on missing session: {e}");
                }
                self.emit_show_login();
                return;
            }
            Err(e) => {
                tracing::warn!("failed to load session: {e}");
                self.emit_show_login();
                self.emit_login_error(&e);
                return;
            }
        };

        self.emit(UiEvent::Status("opening-store".into()));

        let passphrase = match self.get_or_create_passphrase().await {
            Ok(p) => p,
            Err(e) => {
                self.emit_show_login();
                self.emit_login_error(&e);
                return;
            }
        };

        let ui_tx = self.ui_tx.clone();
        let on_progress = Box::new(move |msg| {
            drop(ui_tx.send(UiEvent::Status(msg)));
        });

        if let Err(e) = self
            .matrix
            .restore_session(&session, &passphrase, on_progress)
            .await
        {
            tracing::warn!("session restore failed: {e}");
            self.clear_local_state().await;
            self.emit_show_login();
            self.emit_login_error(&e);
            return;
        }

        tracing::info!(user_id = %session.user_id, "session restore complete");
        self.emit(UiEvent::LoginSuccess {
            user_id: session.user_id,
        });
        self.send_cmd(UiCommand::FetchRooms);
    }

    #[allow(clippy::cognitive_complexity)]
    async fn handle_check_server(&mut self, homeserver: &str) {
        tracing::info!(homeserver, "checking server");
        let passphrase = match self.get_or_create_passphrase().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("failed to get passphrase: {e}");
                self.emit_login_error(&e);
                return;
            }
        };
        match self.matrix.discover_auth(homeserver, &passphrase).await {
            Ok(info) => self.emit(UiEvent::ServerInfo(info)),
            Err(e) => {
                tracing::warn!(homeserver, "server discovery failed: {e}");
                self.emit_login_error(&e);
            }
        }
    }

    async fn handle_login_password(&mut self, creds: LoginCredentials) {
        match self.matrix.login_password(creds).await {
            Ok(session) => {
                tracing::info!(user_id = %session.user_id, "password login succeeded");
                self.save_session(&session).await;
                self.emit(UiEvent::LoginSuccess {
                    user_id: session.user_id,
                });
                self.send_cmd(UiCommand::FetchRooms);
            }
            Err(e) => {
                tracing::warn!("password login failed: {e}");
                self.emit_login_error(&e);
            }
        }
    }

    async fn handle_login_oauth(&mut self) {
        match self.run_oauth_flow().await {
            Ok(()) => {
                self.send_cmd(UiCommand::FetchRooms);
            }
            Err(e) => {
                tracing::warn!("OAuth login failed: {e}");
                self.emit_login_error(&e);
            }
        }
    }

    async fn run_oauth_flow(&mut self) -> Result<()> {
        let oauth_data = self.matrix.login_oauth_start().await?;
        open::that_in_background(&oauth_data.auth_url);
        self.emit(UiEvent::Status("waiting-auth".into()));
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
            self.emit_toast_error(format!(
                "Session not saved. You may need to log in again after restart: {e}"
            ));
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
        self.reset_room_state();
        self.clear_local_state().await;
        self.emit(UiEvent::LoggedOut);
        self.emit(UiEvent::LoginError(
            "Session expired. Please log in again.".into(),
        ));
    }

    #[allow(clippy::cognitive_complexity)]
    async fn handle_logout(&mut self) {
        tracing::info!("user initiated logout");
        self.shutdown_all_tasks().await;
        self.reset_room_state();
        if let Err(e) = self.matrix.logout().await {
            tracing::warn!("failed to logout from server: {e}");
        }
        self.clear_local_state().await;
        tracing::info!("logout complete");
        self.emit(UiEvent::ConnectionStatus(ConnectionStatus::Disconnected));
        self.emit(UiEvent::LoggedOut);
    }

    async fn handle_select_room(&mut self, room_id: RoomId) {
        tracing::info!(%room_id, "switching room");
        self.timeline.reset().await;

        self.viewport = ViewportController::new();
        self.active_room_id = Some(room_id.clone());
        self.at_bottom.store(true, Ordering::Relaxed);
        self.new_messages_counter.store(0, Ordering::Relaxed);

        self.emit(UiEvent::Timeline {
            room_id: room_id.clone(),
            patch: Box::new(TimelinePatch::Clear),
        });

        let (tl_tx, mut tl_rx) = mpsc::unbounded_channel::<TimelinePatch>();
        let (tl_cmd_tx, tl_cmd_rx) = mpsc::unbounded_channel::<TimelineCommand>();
        self.timeline_cmd_tx = Some(tl_cmd_tx);

        let matrix_tl = Arc::clone(&self.matrix);
        let ui_tx = self.ui_tx.clone();
        let token = self.timeline.token();
        let rid = room_id.clone();
        let at_bottom = Arc::clone(&self.at_bottom);
        let new_msgs = Arc::clone(&self.new_messages_counter);

        self.timeline.spawn(async move {
            let subscribe = matrix_tl.subscribe_timeline(&room_id, tl_tx, tl_cmd_rx);
            let forward = async {
                while let Some(patch) = tl_rx.recv().await {
                    tracing::debug!(
                        patch = patch.label(),
                        %rid,
                        "forwarding timeline patch as UiEvent"
                    );

                    if !at_bottom.load(Ordering::Relaxed) {
                        let added = count_appended(&patch);
                        if added > 0 {
                            let prev = new_msgs.fetch_add(added, Ordering::Relaxed);
                            let total = prev.saturating_add(added);
                            ui_tx
                                .send(UiEvent::NewMessagesBadge {
                                    room_id: rid.clone(),
                                    count: total,
                                })
                                .ok();
                        }
                    }

                    let event = UiEvent::Timeline {
                        room_id: rid.clone(),
                        patch: Box::new(patch),
                    };
                    if let Err(e) = ui_tx.send(event) {
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

    fn handle_paginate_backwards(&mut self, room_id: &RoomId) {
        if self.active_room_id.as_ref() != Some(room_id) {
            return;
        }
        if !self.viewport.should_paginate_backwards(true) {
            return;
        }
        self.viewport.set_backwards_loading(true);
        if let Some(tx) = &self.timeline_cmd_tx
            && tx.send(TimelineCommand::PaginateBackwards).is_err()
        {
            tracing::debug!("timeline command channel closed");
        }
    }

    fn handle_paginate_forwards(&mut self, room_id: &RoomId) {
        if self.active_room_id.as_ref() != Some(room_id) {
            return;
        }
        if !self.viewport.should_paginate_forwards(true) {
            return;
        }
        self.viewport.set_forwards_loading(true);
        if let Some(tx) = &self.timeline_cmd_tx
            && tx.send(TimelineCommand::PaginateForwards).is_err()
        {
            tracing::debug!("timeline command channel closed");
        }
    }

    fn handle_jump_to_latest(&mut self, room_id: &RoomId) {
        if self.active_room_id.as_ref() != Some(room_id) {
            return;
        }
        self.viewport.jump_to_latest();
        self.at_bottom.store(true, Ordering::Relaxed);
        self.new_messages_counter.store(0, Ordering::Relaxed);
        self.emit(UiEvent::ScrollToBottom {
            room_id: room_id.clone(),
        });
        self.emit(UiEvent::NewMessagesBadge {
            room_id: room_id.clone(),
            count: 0,
        });
    }

    fn handle_scroll_position_changed(&mut self, at_top: bool, at_bottom: bool) {
        let mode_changed = self.viewport.update_scroll_position(at_top, at_bottom);

        self.at_bottom.store(at_bottom, Ordering::Relaxed);

        let Some(room_id) = self.active_room_id.clone() else {
            return;
        };

        if at_top {
            self.handle_paginate_backwards(&room_id);
        }

        if mode_changed && self.viewport.mode() == ScrollMode::FollowLive {
            self.new_messages_counter.store(0, Ordering::Relaxed);
            self.emit(UiEvent::NewMessagesBadge { room_id, count: 0 });
        }
    }

    async fn handle_send_message(&mut self, room_id: RoomId, body: String) {
        if let Err(e) = self.matrix.send_text(&room_id, &body).await {
            tracing::warn!("failed to enqueue message: {e}");
            self.emit_toast_error(format!("Failed to send message: {e}"));
        }
    }

    fn handle_open_media(&mut self, event_id: String) {
        self.reap_finished();

        let matrix = Arc::clone(&self.matrix);
        let ui_tx = self.ui_tx.clone();
        self.fire_and_forget.spawn(async move {
            match matrix.download_media(&event_id, false).await {
                Ok(data) => {
                    let ext = infer::get(&data).map_or("bin", |t| t.extension());
                    let dir = env::temp_dir().join("utdm-media");
                    if let Err(e) = fs::create_dir_all(&dir).await {
                        tracing::warn!("failed to create temp media dir: {e}");
                        return;
                    }
                    let path = dir.join(format!("{}.{ext}", hex_encode_id(&event_id)));
                    if let Err(e) = fs::write(&path, &data).await {
                        tracing::warn!("failed to write temp media file: {e}");
                        return;
                    }
                    open::that_in_background(&path);
                }
                Err(e) => {
                    ui_tx
                        .send(UiEvent::ToastError(format!(
                            "Failed to download media: {e}"
                        )))
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
                            .send(UiEvent::ToastError(format!("Failed to save file: {e}")))
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
                        .send(UiEvent::ToastError(format!("Failed to download file: {e}")))
                        .ok();
                }
            }
        });
    }

    async fn handle_accept_verification(&mut self) {
        if let Err(e) = self.matrix.accept_verification().await {
            tracing::warn!("verification accept failed: {e}");
            self.emit_toast_error(format!("Verification accept failed: {e}"));
        }
    }

    async fn handle_reject_verification(&mut self) {
        if let Err(e) = self.matrix.reject_verification().await {
            tracing::warn!("verification reject failed: {e}");
            self.emit_toast_error(format!("Verification reject failed: {e}"));
        }
    }

    async fn handle_confirm_verification(&mut self) {
        if let Err(e) = self.matrix.confirm_verification().await {
            tracing::warn!("verification confirm failed: {e}");
            self.emit_toast_error(format!("Verification confirm failed: {e}"));
        }
    }

    async fn handle_fetch_rooms(&mut self) {
        self.connected = true;
        self.emit(UiEvent::Status("syncing".into()));
        self.start_background_listeners().await;
        self.emit(UiEvent::ConnectionStatus(ConnectionStatus::Connecting));
        self.start_sync_pipeline();
    }

    fn handle_rooms_updated(&mut self, rooms: Vec<Room>) {
        if !self.connected {
            return;
        }
        self.all_rooms = rooms;
        self.emit_filtered_rooms();
        self.emit_spaces();
    }

    fn handle_spaces_updated(&mut self, spaces: Vec<Space>) {
        if !self.connected {
            return;
        }
        self.space_children = spaces
            .iter()
            .map(|s| {
                let children: HashSet<String> = s.child_room_ids.iter().cloned().collect();
                (s.id.clone(), children)
            })
            .collect();
        self.spaces = spaces;

        let selection_gone = self
            .selected_space
            .as_ref()
            .is_some_and(|s| !self.space_children.contains_key(s.as_ref()));
        if selection_gone {
            self.selected_space = None;
        }

        self.emit_spaces();
        self.emit_filtered_rooms();
    }

    fn handle_select_space(&mut self, space: Option<RoomId>) {
        self.selected_space = space.filter(|id| !id.is_empty());
        self.emit_filtered_rooms();
    }

    fn emit_filtered_rooms(&self) {
        let filtered = filter_rooms(
            &self.all_rooms,
            &self.space_children,
            self.selected_space.as_deref(),
        );
        self.emit(UiEvent::Rooms(filtered));
    }

    fn emit_spaces(&self) {
        let spaces = aggregate_space_counts(&self.spaces, &self.space_children, &self.all_rooms);
        self.emit(UiEvent::Spaces(spaces));
    }

    fn reset_room_state(&mut self) {
        self.connected = false;
        self.all_rooms.clear();
        self.spaces.clear();
        self.space_children.clear();
        self.selected_space = None;
    }

    async fn start_background_listeners(&mut self) {
        self.background.reset().await;
        Self::spawn_session_persister(
            &mut self.background,
            &self.matrix,
            &self.storage,
            &self.ui_tx,
        );
        Self::spawn_verification_forwarder(&mut self.background, &self.matrix, &self.ui_tx);
    }

    fn spawn_session_persister(
        group: &mut TaskGroup,
        matrix: &Arc<dyn MatrixPort>,
        storage: &Arc<dyn StoragePort>,
        ui_tx: &mpsc::UnboundedSender<UiEvent>,
    ) {
        let matrix = Arc::clone(matrix);
        let storage = Arc::clone(storage);
        let ui_tx = ui_tx.clone();
        let token = group.token();
        group.spawn(async move {
            let (session_tx, mut session_rx) = mpsc::unbounded_channel::<Session>();
            let subscribe = matrix.subscribe_session_changes(session_tx);
            let persist = async {
                while let Some(session) = session_rx.recv().await {
                    if let Err(e) = storage.save_session(&session).await {
                        tracing::warn!("failed to persist refreshed session: {e}");
                        ui_tx
                            .send(UiEvent::ToastError(format!(
                                "Failed to save refreshed session: {e}"
                            )))
                            .ok();
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
        let matrix = Arc::clone(&self.matrix);
        let ui_tx = self.ui_tx.clone();
        let cmd_tx = self.cmd_tx.clone();
        let token = self.background.token();

        let on_sync: Box<dyn Fn(SyncEvent) + Send + Sync> = Box::new(move |event| match event {
            SyncEvent::Connected => {
                ui_tx
                    .send(UiEvent::ConnectionStatus(ConnectionStatus::Connected))
                    .ok();
            }
            SyncEvent::Rooms(rooms) => {
                cmd_tx.send(UiCommand::RoomsUpdated(rooms)).ok();
            }
            SyncEvent::Spaces(spaces) => {
                cmd_tx.send(UiCommand::SpacesUpdated(spaces)).ok();
            }
            SyncEvent::ConnectionError(msg) => {
                ui_tx
                    .send(UiEvent::ConnectionStatus(ConnectionStatus::Error(msg)))
                    .ok();
            }
            SyncEvent::SessionExpired => {
                cmd_tx.send(UiCommand::SessionExpired).ok();
            }
        });

        self.background.spawn(async move {
            tokio::select! {
                result = matrix.start_sync(on_sync) => {
                    if let Err(e) = result {
                        tracing::error!("sync loop ended with error: {e}");
                    }
                }
                () = token.cancelled() => {
                    tracing::debug!("sync task cancelled");
                }
            }
        });
    }
}

fn count_appended(patch: &TimelinePatch) -> u32 {
    match patch {
        TimelinePatch::Append(msgs) => msgs.len().try_into().unwrap_or(u32::MAX),
        TimelinePatch::PushBack(_) => 1,
        TimelinePatch::Batch(patches) => patches.iter().map(count_appended).sum(),
        _ => 0,
    }
}

fn filter_rooms(
    all_rooms: &[Room],
    space_children: &HashMap<String, HashSet<String>>,
    selected: Option<&str>,
) -> Vec<Room> {
    match selected {
        None => all_rooms.to_vec(),
        Some(space_id) => match space_children.get(space_id) {
            Some(children) => all_rooms
                .iter()
                .filter(|r| children.contains(&*r.id))
                .cloned()
                .collect(),
            None => Vec::new(),
        },
    }
}

fn aggregate_space_counts(
    spaces: &[Space],
    space_children: &HashMap<String, HashSet<String>>,
    all_rooms: &[Room],
) -> Vec<Space> {
    spaces
        .iter()
        .map(|space| {
            let (unread, mentions) = match space_children.get(&space.id) {
                Some(children) => all_rooms
                    .iter()
                    .filter(|r| children.contains(&*r.id))
                    .fold((0u64, 0u64), |(unread, mentions), r| {
                        (unread + r.unread_count, mentions + r.mention_count)
                    }),
                None => (0, 0),
            };
            Space {
                unread,
                mentions,
                ..space.clone()
            }
        })
        .collect()
}
