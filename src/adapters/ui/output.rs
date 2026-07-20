use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, watch};

use crate::commands::{AppViewState, Effect, LoginStep};
use crate::domain::models::{
    ConnectionStatus, LoginMethod, PaginationState, Room, RoomId, ServerInfo, Space, TimelinePatch,
    TimelineStatus, VerificationEvent,
};
use crate::ports::output::AppOutputPort;

pub struct UiEventOutput {
    ui_tx: mpsc::Sender<Effect>,
    view_tx: watch::Sender<Arc<AppViewState>>,
}

impl UiEventOutput {
    pub fn new(ui_tx: mpsc::Sender<Effect>, view_tx: watch::Sender<Arc<AppViewState>>) -> Self {
        Self { ui_tx, view_tx }
    }

    async fn emit(&self, event: Effect) {
        if let Err(e) = self.ui_tx.send(event).await {
            tracing::debug!("failed to send UI event: {e}");
        }
    }

    fn publish(&self, mutate: impl FnOnce(&mut AppViewState)) {
        self.view_tx.send_modify(|snapshot| {
            let mut next = (**snapshot).clone();
            mutate(&mut next);
            *snapshot = Arc::new(next);
        });
    }

    fn clear_login_status(&self) {
        if let Err(e) = self.ui_tx.try_send(Effect::Status(String::new())) {
            tracing::debug!("failed to clear status: {e}");
        }
    }
}

#[async_trait]
impl AppOutputPort for UiEventOutput {
    async fn login_error(&self, message: String) {
        self.emit(Effect::LoginError(message)).await;
    }

    async fn notify_error(&self, message: String) {
        self.emit(Effect::Toast(message)).await;
    }

    async fn selected_room(&self, id: RoomId, name: String, member_count: u64, generation: i32) {
        self.emit(Effect::SelectedRoom {
            id,
            name,
            member_count,
            generation,
        })
        .await;
    }

    async fn timeline(&self, room_id: RoomId, generation: i32, patch: Box<TimelinePatch>) {
        self.emit(Effect::Timeline {
            room_id,
            generation,
            patch,
        })
        .await;
    }

    async fn timeline_status(&self, room_id: RoomId, generation: i32, status: TimelineStatus) {
        self.emit(Effect::TimelineStatus {
            room_id,
            generation,
            status,
        })
        .await;
    }

    async fn verification(&self, event: VerificationEvent) {
        self.emit(Effect::Verification(event)).await;
    }

    async fn file_saved(&self, path: String) {
        self.emit(Effect::FileSaved { path }).await;
    }

    async fn logged_out(&self) {
        self.emit(Effect::LoggedOut).await;
    }

    fn server_info(&self, info: ServerInfo) {
        let method = LoginMethod::from_auth_methods(&info.auth_methods);
        self.publish(|view| {
            view.lifecycle.method = method;
            view.lifecycle.resolved_homeserver = info.homeserver_url;
            view.lifecycle.step = LoginStep::Credentials;
        });
        self.clear_login_status();
    }

    fn show_login(&self) {
        self.publish(|view| view.lifecycle.step = LoginStep::Homeserver);
        self.clear_login_status();
    }

    fn login_success(&self, user_id: String) {
        self.publish(|view| {
            view.lifecycle.user_id = user_id;
            view.lifecycle.step = LoginStep::LoggedIn;
        });
        self.clear_login_status();
    }

    fn user_avatar(&self, path: Option<PathBuf>) {
        self.publish(|view| view.lifecycle.avatar_path = path);
    }

    fn selected_space(&self, id: String) {
        self.publish(|view| view.directory.space_id = id);
    }

    fn selected_subspace(&self, id: String) {
        self.publish(|view| view.directory.subspace_id = id);
    }

    fn pagination_state(&self, generation: i32, state: PaginationState) {
        self.publish(|view| {
            view.pagination.retarget(generation);
            view.pagination.backwards_loading = state.backwards_loading;
            view.pagination.forwards_loading = state.forwards_loading;
        });
    }

    fn new_messages_badge(&self, generation: i32, count: u32) {
        self.publish(|view| {
            view.pagination.retarget(generation);
            view.pagination.new_messages = count;
        });
    }

    fn scroll_to_bottom(&self, generation: i32) {
        self.publish(|view| {
            view.pagination.retarget(generation);
            view.pagination.new_messages = 0;
        });
    }

    fn rooms(&self, rooms: Arc<[Room]>) {
        self.publish(|view| view.directory.rooms = rooms);
    }

    fn spaces(&self, spaces: Arc<[Space]>) {
        self.publish(|view| view.directory.spaces = spaces);
    }

    fn subspaces(&self, spaces: Arc<[Space]>) {
        self.publish(|view| view.directory.subspaces = spaces);
    }

    fn connection_status(&self, status: ConnectionStatus) {
        self.publish(|view| view.connection = status);
    }

    fn status(&self, message: String) {
        if let Err(e) = self.ui_tx.try_send(Effect::Status(message)) {
            tracing::debug!("failed to send status: {e}");
        }
    }
}
