use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::domain::models::{
    LoginCredentials, OAuthLoginData, RoomId, ServerInfo, Session, SyncEvent, TimelineCommand,
    TimelineUpdate, VerificationEvent,
};
use crate::error::Result;

pub struct AuthenticatedSession {
    pub session: Session,
    pub sync: Arc<dyn SyncPort>,
    pub timeline: Arc<dyn TimelinePort>,
    pub media: Arc<dyn MediaPort>,
    pub verification: Arc<dyn VerificationPort>,
    pub lifecycle: Arc<dyn SessionPort>,
}

#[async_trait]
pub trait AuthPort: Send + Sync {
    async fn discover_auth(&self, homeserver: &str, passphrase: &str) -> Result<ServerInfo>;
    async fn login_password(&self, creds: LoginCredentials) -> Result<AuthenticatedSession>;
    async fn login_oauth_start(&self) -> Result<OAuthLoginData>;
    async fn login_oauth_finish(&self) -> Result<AuthenticatedSession>;
    async fn cancel_oauth(&self);
    async fn restore_session(
        &self,
        session: &Session,
        passphrase: &str,
        on_progress: Box<dyn Fn(String) + Send + Sync>,
    ) -> Result<AuthenticatedSession>;
}

#[async_trait]
pub trait SyncPort: Send + Sync {
    async fn start_sync(
        &self,
        on_sync: Box<dyn Fn(SyncEvent) + Send + Sync>,
        cancel: CancellationToken,
    ) -> Result<()>;
}

#[async_trait]
pub trait TimelinePort: Send + Sync {
    async fn subscribe_timeline(
        &self,
        room_id: &RoomId,
        timeline_tx: mpsc::Sender<TimelineUpdate>,
        cmd_rx: mpsc::UnboundedReceiver<TimelineCommand>,
    ) -> Result<()>;
    async fn send_text(&self, room_id: &RoomId, body: &str) -> Result<()>;
    async fn send_reply(&self, room_id: &RoomId, body: &str, in_reply_to: &str) -> Result<()>;
}

#[async_trait]
pub trait MediaPort: Send + Sync {
    async fn download_media(&self, event_id: &str, thumbnail: bool) -> Result<Vec<u8>>;
}

#[async_trait]
pub trait VerificationPort: Send + Sync {
    async fn listen_for_verification(
        &self,
        tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Result<()>;
    async fn accept_verification(&self) -> Result<()>;
    async fn confirm_verification(&self) -> Result<()>;
    async fn reject_verification(&self) -> Result<()>;
}

#[async_trait]
pub trait SessionPort: Send + Sync {
    async fn subscribe_session_changes(
        &self,
        session_tx: mpsc::UnboundedSender<Session>,
    ) -> Result<()>;
    async fn fetch_user_avatar(&self) -> Result<Option<PathBuf>>;
    async fn logout(&self) -> Result<()>;
    async fn clear_store(&self) -> Result<()>;
}
