use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::domain::account::AccountScope;
use crate::domain::auth::{LoginCredentials, OAuthLoginData, ServerInfo, Session};
use crate::domain::media::{MediaRendition, OutgoingAttachment, WaveformNeed};
use crate::domain::message::PinnedMessage;
use crate::domain::room::RoomId;
use crate::domain::space_index::HierarchyPage;
use crate::domain::sticker::{PackId, StickerPack};
use crate::domain::sync::{SyncEvent, SyncOutcome};
use crate::domain::timeline::{TimelineCommand, TimelineFocus, TimelineUpdate};
use crate::domain::verification::VerificationEvent;
use crate::error::Result;

pub type SyncSink = Arc<dyn Fn(SyncEvent) + Send + Sync>;
pub type ProgressSink = Box<dyn Fn(RestoreStep) + Send + Sync>;

#[derive(Clone, Copy)]
pub enum RestoreStep {
    Connecting,
    RestoringAuth,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoginResolution {
    RollBack,
    RollForward,
    Close,
}

pub struct PendingLogin {
    pub txn: String,
    pub account: AccountScope,
    pub resolution: LoginResolution,
    pub credentials_staged: bool,
}

pub enum InterruptedLogin {
    Journaled(PendingLogin),
    Unreadable { txn: String, reason: String },
}

impl InterruptedLogin {
    pub fn txn(&self) -> &str {
        match self {
            Self::Journaled(login) => &login.txn,
            Self::Unreadable { txn, .. } => txn,
        }
    }
}

pub enum LocalDataOwnership {
    Exclusive,
    AnotherInstance { lock: String },
    Undetermined { reason: String },
}

#[derive(Debug, Default)]
pub struct CleanupReport {
    pub quarantined: Vec<PathBuf>,
    pub failures: Vec<String>,
}

impl CleanupReport {
    pub fn is_clean(&self) -> bool {
        self.quarantined.is_empty() && self.failures.is_empty()
    }

    pub fn is_quarantined_only(&self) -> bool {
        self.failures.is_empty() && !self.quarantined.is_empty()
    }

    pub fn has_failures(&self) -> bool {
        !self.failures.is_empty()
    }

    pub fn fail(&mut self, detail: impl Into<String>) {
        self.failures.push(detail.into());
    }

    pub fn merge(&mut self, other: Self) {
        self.quarantined.extend(other.quarantined);
        self.failures.extend(other.failures);
    }

    pub fn summary(&self) -> String {
        let mut parts = self.failures.clone();
        parts.extend(
            self.quarantined
                .iter()
                .map(|p| format!("{} could not be deleted", p.display())),
        );
        parts.join("; ")
    }

    pub fn quarantined_paths(&self) -> String {
        self.quarantined
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("; ")
    }
}

pub struct AuthenticatedSession {
    pub session: Session,
    pub sync: Arc<dyn SyncPort>,
    pub timeline: Arc<dyn TimelinePort>,
    pub pinned: Arc<dyn PinnedPort>,
    pub media: Arc<dyn MediaPort>,
    pub verification: Arc<dyn VerificationPort>,
    pub space_order: Arc<dyn SpaceOrderPort>,
    pub space_index: Arc<dyn SpaceIndexPort>,
    pub stickers: Arc<dyn StickerPort>,
    pub lifecycle: Arc<dyn SessionPort>,
}

#[async_trait]
pub trait AuthPort: Send + Sync {
    async fn discover_auth(&self, homeserver: &str, passphrase: &str) -> Result<ServerInfo>;
    async fn login_password(&self, creds: LoginCredentials) -> Result<Session>;
    async fn login_oauth_start(&self) -> Result<OAuthLoginData>;
    async fn login_oauth_finish(&self) -> Result<Session>;
    async fn cancel_oauth(&self);
    async fn adopt_session(
        &self,
        session: &Session,
        passphrase: &str,
    ) -> Result<Box<dyn StoreAdoption>>;
    async fn restore_session(
        &self,
        session: &Session,
        passphrase: &str,
        on_progress: ProgressSink,
    ) -> Result<AuthenticatedSession>;
    async fn reauthenticate(
        &self,
        prior: &Session,
        passphrase: &str,
        creds: LoginCredentials,
    ) -> Result<AuthenticatedSession>;
    async fn reauth_oauth_start(&self, prior: &Session, passphrase: &str)
    -> Result<OAuthLoginData>;
    async fn reauth_oauth_finish(&self, prior: &Session) -> Result<AuthenticatedSession>;
    fn local_data_ownership(&self) -> LocalDataOwnership;
    async fn interrupted_logins(&self) -> Result<Vec<InterruptedLogin>>;
    async fn unwind_login(&self, txn: &str) -> CleanupReport;
    async fn settle_login(&self, txn: &str) -> CleanupReport;
    async fn mark_rolled_back(&self, txn: &str) -> Result<()>;
    async fn forget_login(&self, txn: &str);
}

#[async_trait]
pub trait StoreAdoption: Send + Sync {
    fn transaction(&self) -> &str;
    async fn credentials_staged(&self) -> Result<()>;
    async fn credentials_written(&self) -> Result<()>;
    async fn rolling_back(&self) -> Result<()>;
    async fn commit(self: Box<Self>) -> AuthenticatedSession;
    async fn unwind(self: Box<Self>) -> CleanupReport;
}

#[async_trait]
pub trait SyncPort: Send + Sync {
    async fn start_sync(&self, on_sync: SyncSink, cancel: CancellationToken) -> SyncOutcome;
    fn set_selected_room(&self, room_id: Option<&RoomId>);
}

#[async_trait]
pub trait SpaceOrderPort: Send + Sync {
    async fn set_space_order(&self, space_id: &RoomId, order: &str) -> Result<()>;
}

#[async_trait]
pub trait SpaceIndexPort: Send + Sync {
    async fn hierarchy_page(&self, space_id: &RoomId, from: Option<&str>) -> Result<HierarchyPage>;
    async fn join(&self, room_id: &RoomId, via: &[String]) -> Result<()>;
    async fn fetch_avatars(&self, mxcs: &[String]) -> usize;
}

#[async_trait]
pub trait TimelinePort: Send + Sync {
    async fn subscribe_timeline(
        &self,
        room_id: &RoomId,
        focus: TimelineFocus,
        timeline_tx: mpsc::Sender<TimelineUpdate>,
        cmd_rx: mpsc::UnboundedReceiver<TimelineCommand>,
    ) -> Result<()>;
    async fn send_text(&self, room_id: &RoomId, body: &str) -> Result<()>;
    async fn send_reply(&self, room_id: &RoomId, body: &str, in_reply_to: &str) -> Result<()>;
    async fn send_attachment(
        &self,
        room_id: &RoomId,
        attachment: &OutgoingAttachment,
    ) -> Result<()>;
    async fn resend(&self, room_id: &RoomId, local_id: &str) -> Result<()>;
    async fn discard_send(&self, room_id: &RoomId, local_id: &str) -> Result<()>;
}

#[async_trait]
pub trait PinnedPort: Send + Sync {
    async fn subscribe_pinned(
        &self,
        room_id: &RoomId,
        pinned_tx: mpsc::Sender<Vec<PinnedMessage>>,
    ) -> Result<()>;
}

#[async_trait]
pub trait MediaPort: Send + Sync {
    async fn download_media(
        &self,
        room_id: &RoomId,
        event_id: &str,
        rendition: MediaRendition,
    ) -> Result<Vec<u8>>;
    async fn materialize_video(&self, room_id: &RoomId, event_id: &str) -> Result<PathBuf>;
    async fn materialize_audio(
        &self,
        room_id: &RoomId,
        event_id: &str,
        need: WaveformNeed,
    ) -> Result<PathBuf>;
}

#[derive(Debug, Default)]
pub struct StickerCatalog {
    pub packs: Vec<StickerPack>,
    pub room_encrypted: bool,
}

#[async_trait]
pub trait StickerPort: Send + Sync {
    async fn catalog(&self, room_id: &RoomId) -> Result<StickerCatalog>;
    async fn prefetch(&self, mxcs: &[String]) -> usize;
    async fn send_sticker(
        &self,
        room_id: &RoomId,
        pack: &PackId,
        shortcode: &str,
        in_reply_to: Option<&str>,
    ) -> Result<()>;
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
    async fn suspend(&self);
    async fn clear_store(&self) -> CleanupReport;
}
