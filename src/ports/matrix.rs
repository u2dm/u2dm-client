use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::domain::models::{
    LoginCredentials, OAuthLoginData, Room, RoomId, ServerInfo, Session, SyncEvent, TimelinePatch,
    VerificationEvent,
};
use crate::error::Result;

#[async_trait]
pub trait MatrixPort: Send + Sync {
    async fn discover_auth(&self, homeserver: &str, passphrase: &str) -> Result<ServerInfo>;
    async fn login_password(&self, creds: LoginCredentials) -> Result<Session>;
    async fn login_oauth_start(&self) -> Result<OAuthLoginData>;
    async fn login_oauth_finish(&self) -> Result<Session>;
    async fn rooms(&self) -> Result<Vec<Room>>;
    async fn subscribe_timeline(
        &self,
        room_id: &RoomId,
        timeline_tx: mpsc::UnboundedSender<TimelinePatch>,
    ) -> Result<()>;
    async fn start_sync(&self, on_sync: Box<dyn Fn(SyncEvent) + Send + Sync>) -> Result<()>;
    async fn send_text(&self, room_id: &RoomId, body: &str) -> Result<()>;
    async fn download_media(&self, event_id: &str, thumbnail: bool) -> Result<Vec<u8>>;
    async fn restore_session(&self, session: &Session, passphrase: &str) -> Result<()>;
    async fn logout(&self) -> Result<()>;
    async fn clear_store(&self) -> Result<()>;
    async fn listen_for_verification(
        &self,
        tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Result<()>;
    async fn accept_verification(&self) -> Result<()>;
    async fn confirm_verification(&self) -> Result<()>;
    async fn reject_verification(&self) -> Result<()>;
    async fn subscribe_session_changes(
        &self,
        session_tx: mpsc::UnboundedSender<Session>,
    ) -> Result<()>;
}
