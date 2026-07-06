use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::commands::UiEvent;
use crate::domain::models::{
    ConnectionStatus, PaginationState, Room, RoomId, ServerInfo, Space, TimelinePatch,
    VerificationEvent,
};
use crate::ports::output::AppOutputPort;

pub struct UiEventOutput {
    ui_tx: mpsc::UnboundedSender<UiEvent>,
}

impl UiEventOutput {
    pub fn new(ui_tx: mpsc::UnboundedSender<UiEvent>) -> Self {
        Self { ui_tx }
    }

    fn emit(&self, event: UiEvent) {
        if let Err(e) = self.ui_tx.send(event) {
            tracing::debug!("failed to send UI event: {e}");
        }
    }
}

impl AppOutputPort for UiEventOutput {
    fn server_info(&self, info: ServerInfo) {
        self.emit(UiEvent::ServerInfo(info));
    }

    fn show_login(&self) {
        self.emit(UiEvent::ShowLogin);
    }

    fn login_success(&self, user_id: String) {
        self.emit(UiEvent::LoginSuccess { user_id });
    }

    fn user_avatar(&self, path: Option<PathBuf>) {
        self.emit(UiEvent::UserAvatar(path));
    }

    fn login_error(&self, message: String) {
        self.emit(UiEvent::LoginError(message));
    }

    fn notify_error(&self, message: String) {
        self.emit(UiEvent::ToastError(message));
    }

    fn status(&self, message: String) {
        self.emit(UiEvent::Status(message));
    }

    fn rooms(&self, rooms: Vec<Room>) {
        self.emit(UiEvent::Rooms(rooms));
    }

    fn spaces(&self, spaces: Vec<Space>) {
        self.emit(UiEvent::Spaces(spaces));
    }

    fn timeline(&self, room_id: RoomId, patch: Box<TimelinePatch>) {
        self.emit(UiEvent::Timeline { room_id, patch });
    }

    fn pagination_state(&self, room_id: RoomId, state: PaginationState) {
        self.emit(UiEvent::PaginationState { room_id, state });
    }

    fn new_messages_badge(&self, room_id: RoomId, count: u32) {
        self.emit(UiEvent::NewMessagesBadge { room_id, count });
    }

    fn scroll_to_bottom(&self, room_id: RoomId) {
        self.emit(UiEvent::ScrollToBottom { room_id });
    }

    fn connection_status(&self, status: ConnectionStatus) {
        self.emit(UiEvent::ConnectionStatus(status));
    }

    fn verification(&self, event: VerificationEvent) {
        self.emit(UiEvent::Verification(event));
    }

    fn file_saved(&self, path: String) {
        self.emit(UiEvent::FileSaved { path });
    }

    fn logged_out(&self) {
        self.emit(UiEvent::LoggedOut);
    }
}
