use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::domain::models::{
    ConnectionStatus, PaginationState, Room, RoomId, ServerInfo, Space, TimelinePatch,
    TimelineStatus, VerificationEvent,
};

#[async_trait]
pub trait AppOutputPort: Send + Sync {
    async fn login_error(&self, message: String);
    async fn notify_error(&self, message: String);
    async fn selected_room(&self, id: RoomId, name: String, member_count: u64, generation: i32);
    async fn timeline(&self, room_id: RoomId, generation: i32, patch: Box<TimelinePatch>);
    async fn timeline_status(&self, room_id: RoomId, generation: i32, status: TimelineStatus);
    async fn verification(&self, event: VerificationEvent);
    async fn file_saved(&self, path: String);
    async fn logged_out(&self);

    fn server_info(&self, info: ServerInfo);
    fn show_login(&self);
    fn login_success(&self, user_id: String);
    fn user_avatar(&self, path: Option<PathBuf>);
    fn selected_space(&self, id: String);
    fn selected_subspace(&self, id: String);
    fn pagination_state(&self, generation: i32, state: PaginationState);
    fn new_messages_badge(&self, generation: i32, count: u32);
    fn scroll_to_bottom(&self, generation: i32);
    fn rooms(&self, rooms: Arc<[Room]>);
    fn spaces(&self, spaces: Arc<[Space]>);
    fn subspaces(&self, spaces: Arc<[Space]>);
    fn connection_status(&self, status: ConnectionStatus);
    fn status(&self, message: String);
}
