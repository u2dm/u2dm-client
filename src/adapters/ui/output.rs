use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, watch};

use crate::commands::UiEvent;
use crate::domain::models::{
    ConnectionStatus, PaginationState, Room, RoomId, ServerInfo, Space, TimelinePatch,
    TimelineStatus, VerificationEvent,
};
use crate::ports::output::AppOutputPort;

#[allow(clippy::struct_field_names)]
pub struct UiEventOutput {
    ui_tx: mpsc::Sender<UiEvent>,
    rooms_tx: watch::Sender<Arc<[Room]>>,
    spaces_tx: watch::Sender<Arc<[Space]>>,
    subspaces_tx: watch::Sender<Arc<[Space]>>,
    connection_tx: watch::Sender<ConnectionStatus>,
    status_tx: watch::Sender<String>,
}

impl UiEventOutput {
    pub fn new(
        ui_tx: mpsc::Sender<UiEvent>,
        rooms_tx: watch::Sender<Arc<[Room]>>,
        spaces_tx: watch::Sender<Arc<[Space]>>,
        subspaces_tx: watch::Sender<Arc<[Space]>>,
        connection_tx: watch::Sender<ConnectionStatus>,
        status_tx: watch::Sender<String>,
    ) -> Self {
        Self {
            ui_tx,
            rooms_tx,
            spaces_tx,
            subspaces_tx,
            connection_tx,
            status_tx,
        }
    }

    async fn emit(&self, event: UiEvent) {
        if let Err(e) = self.ui_tx.send(event).await {
            tracing::debug!("failed to send UI event: {e}");
        }
    }
}

#[async_trait]
impl AppOutputPort for UiEventOutput {
    async fn server_info(&self, info: ServerInfo) {
        self.emit(UiEvent::ServerInfo(info)).await;
    }

    async fn show_login(&self) {
        self.emit(UiEvent::ShowLogin).await;
    }

    async fn login_success(&self, user_id: String) {
        self.emit(UiEvent::LoginSuccess { user_id }).await;
    }

    async fn user_avatar(&self, path: Option<PathBuf>) {
        self.emit(UiEvent::UserAvatar(path)).await;
    }

    async fn login_error(&self, message: String) {
        self.emit(UiEvent::LoginError(message)).await;
    }

    async fn notify_error(&self, message: String) {
        self.emit(UiEvent::ToastError(message)).await;
    }

    async fn selected_room(&self, id: RoomId, name: String, member_count: u64) {
        self.emit(UiEvent::SelectedRoom {
            id,
            name,
            member_count,
        })
        .await;
    }

    async fn selected_space(&self, id: String) {
        self.emit(UiEvent::SelectedSpace(id)).await;
    }

    async fn selected_subspace(&self, id: String) {
        self.emit(UiEvent::SelectedSubspace(id)).await;
    }

    async fn timeline(&self, room_id: RoomId, patch: Box<TimelinePatch>) {
        self.emit(UiEvent::Timeline { room_id, patch }).await;
    }

    async fn timeline_status(&self, room_id: RoomId, status: TimelineStatus) {
        self.emit(UiEvent::TimelineStatus { room_id, status }).await;
    }

    async fn pagination_state(&self, room_id: RoomId, state: PaginationState) {
        self.emit(UiEvent::PaginationState { room_id, state }).await;
    }

    async fn new_messages_badge(&self, room_id: RoomId, count: u32) {
        self.emit(UiEvent::NewMessagesBadge { room_id, count })
            .await;
    }

    async fn scroll_to_bottom(&self, room_id: RoomId) {
        self.emit(UiEvent::ScrollToBottom { room_id }).await;
    }

    async fn verification(&self, event: VerificationEvent) {
        self.emit(UiEvent::Verification(event)).await;
    }

    async fn file_saved(&self, path: String) {
        self.emit(UiEvent::FileSaved { path }).await;
    }

    async fn logged_out(&self) {
        self.emit(UiEvent::LoggedOut).await;
    }

    fn rooms(&self, rooms: Arc<[Room]>) {
        drop(self.rooms_tx.send(rooms));
    }

    fn spaces(&self, spaces: Arc<[Space]>) {
        drop(self.spaces_tx.send(spaces));
    }

    fn subspaces(&self, spaces: Arc<[Space]>) {
        drop(self.subspaces_tx.send(spaces));
    }

    fn connection_status(&self, status: ConnectionStatus) {
        drop(self.connection_tx.send(status));
    }

    fn status(&self, message: String) {
        drop(self.status_tx.send(message));
    }
}
