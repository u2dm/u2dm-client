use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::domain::models::{
    LoginCredentials, OAuthLoginData, Room, RoomId, ServerInfo, Session, SyncSnapshot,
    TimelineMessage,
};
use crate::error::Result;

#[async_trait]
pub trait MatrixPort: Send + Sync {
    async fn discover_auth(&self, homeserver: &str) -> Result<ServerInfo>;
    async fn login_password(&self, creds: LoginCredentials) -> Result<Session>;
    async fn login_oauth_start(&self) -> Result<OAuthLoginData>;
    async fn login_oauth_finish(&self) -> Result<Session>;
    async fn rooms(&self) -> Result<Vec<Room>>;
    async fn subscribe_timeline(
        &self,
        room_id: &RoomId,
        timeline_tx: mpsc::Sender<Vec<TimelineMessage>>,
    ) -> Result<()>;
    async fn start_sync(&self, state_tx: mpsc::Sender<SyncSnapshot>) -> Result<()>;
}
