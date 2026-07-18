use std::path::PathBuf;

use crate::domain::models::{
    ConnectionStatus, PaginationState, Room, RoomId, ServerInfo, Space, TimelinePatch,
    TimelineStatus, VerificationEvent,
};

pub trait AppOutputPort: Send + Sync {
    fn server_info(&self, info: ServerInfo);
    fn show_login(&self);
    fn login_success(&self, user_id: String);
    fn user_avatar(&self, path: Option<PathBuf>);
    fn login_error(&self, message: String);
    fn notify_error(&self, message: String);
    fn status(&self, message: String);
    fn rooms(&self, rooms: Vec<Room>);
    fn spaces(&self, spaces: Vec<Space>);
    fn subspaces(&self, spaces: Vec<Space>);
    fn timeline(&self, room_id: RoomId, patch: Box<TimelinePatch>);
    fn timeline_status(&self, room_id: RoomId, status: TimelineStatus);
    fn pagination_state(&self, room_id: RoomId, state: PaginationState);
    fn new_messages_badge(&self, room_id: RoomId, count: u32);
    fn scroll_to_bottom(&self, room_id: RoomId);
    fn connection_status(&self, status: ConnectionStatus);
    fn verification(&self, event: VerificationEvent);
    fn file_saved(&self, path: String);
    fn logged_out(&self);
}
