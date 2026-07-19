use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::domain::models::{
    ConnectionStatus, PaginationState, Room, RoomId, ServerInfo, Space, TimelinePatch,
    TimelineStatus, VerificationEvent,
};

#[async_trait]
pub trait AppOutputPort: Send + Sync {
    async fn server_info(&self, info: ServerInfo);
    async fn show_login(&self);
    async fn login_success(&self, user_id: String);
    async fn user_avatar(&self, path: Option<PathBuf>);
    async fn login_error(&self, message: String);
    async fn notify_error(&self, message: String);
    async fn selected_room(&self, id: RoomId, name: String, member_count: u64, generation: i32);
    async fn selected_space(&self, id: String);
    async fn selected_subspace(&self, id: String);
    async fn timeline(&self, room_id: RoomId, patch: Box<TimelinePatch>);
    async fn timeline_status(&self, room_id: RoomId, status: TimelineStatus);
    async fn pagination_state(&self, room_id: RoomId, state: PaginationState);
    async fn new_messages_badge(&self, room_id: RoomId, count: u32);
    async fn scroll_to_bottom(&self, room_id: RoomId);
    async fn verification(&self, event: VerificationEvent);
    async fn file_saved(&self, path: String);
    async fn logged_out(&self);

    fn rooms(&self, rooms: Arc<[Room]>);
    fn spaces(&self, spaces: Arc<[Space]>);
    fn subspaces(&self, spaces: Arc<[Space]>);
    fn connection_status(&self, status: ConnectionStatus);
    fn status(&self, message: String);
}
