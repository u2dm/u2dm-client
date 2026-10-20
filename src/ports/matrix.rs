use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::domain::account::AccountScope;
use crate::domain::auth::{LoginCredentials, OAuthLoginData, ServerInfo, Session};
use crate::domain::call::ring::{CallSignal, OutgoingRing, RingId};
use crate::domain::call::sfu::{GrantRequest, GrantResult};
use crate::domain::call::{
    DelayedLeave, DeviceStanding, KeySendReport, MemberKey, OutgoingKey, OwnMembership,
    RestartResult, ScheduleResult, SentMembership, Transport,
};
use crate::domain::capabilities::{AccountFacts, AccountPage};
use crate::domain::departure::AdminStanding;
use crate::domain::emote::Emote;
use crate::domain::encryption::{
    AccountEncryption, IdentityOutcome, KeyImport, KeyPassphrase, RecoveryAttempt, RecoveryKey,
    RecoverySetup,
};
use crate::domain::identity::{OwnIdentity, RoomAlert};
use crate::domain::image_pack::{ImagePack, PackId};
use crate::domain::invite_people::{InviteResult, SearchResult};
use crate::domain::link::LauncherSafeUrl;
use crate::domain::matrix_link::RoomAddress;
use crate::domain::media::{MediaRendition, OutgoingAttachment, WaveformNeed};
use crate::domain::member_name::NamedUser;
use crate::domain::message::{MessageEdit, PinChange, PinnedMessage};
use crate::domain::poll::PollDraft;
use crate::domain::presence::{Availability, BeaconResult, LookupResult};
use crate::domain::push::PushNotice;
use crate::domain::room::{NotifyMode, RoomId};
use crate::domain::room_creation::{CreateResult, RoomBlueprint};
use crate::domain::room_info::{Reader, RoomAbout, RosterMember};
use crate::domain::room_preview::PreviewResult;
use crate::domain::sessions::{LogoutRisks, OwnSessions};
use crate::domain::space_index::HierarchyPage;
use crate::domain::sync::{SyncEvent, SyncOutcome};
use crate::domain::timeline::{TimelineCommand, TimelineFocus, TimelineUpdate};
use crate::domain::uia::{AccountPassword, AuthAnswer, Guarded};
use crate::domain::user_info::{
    GlobalProfile, IgnoreChange, Moderation, UserId, UserProfile, UserTrust,
};
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
pub enum AttachmentHandoff {
    Queued,
    Abandoned,
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeviceRemoval {
    Confirmed,
    Unconfirmed,
}

pub enum RestoreOutcome {
    Restored(Box<AuthenticatedSession>),
    Diverged(DeviceRemoval),
}

pub struct AuthenticatedSession {
    pub session: Session,
    pub sync: Arc<dyn SyncPort>,
    pub timeline: Arc<dyn TimelinePort>,
    pub pinned: Arc<dyn PinnedPort>,
    pub media: Arc<dyn MediaPort>,
    pub verification: Arc<dyn VerificationPort>,
    pub identity: Arc<dyn IdentityPort>,
    pub encryption: Arc<dyn EncryptionPort>,
    pub space_order: Arc<dyn SpaceOrderPort>,
    pub space_index: Arc<dyn SpaceIndexPort>,
    pub invites: Arc<dyn InvitePort>,
    pub leaves: Arc<dyn LeavePort>,
    pub upgrades: Arc<dyn UpgradePort>,
    pub room_previews: Arc<dyn RoomPreviewPort>,
    pub room_creation: Arc<dyn RoomCreationPort>,
    pub invite_people: Arc<dyn InvitePeoplePort>,
    pub typing: Arc<dyn TypingPort>,
    pub presence: Arc<dyn PresencePort>,
    pub call_signals: Arc<dyn CallSignalPort>,
    pub rtc_membership: Arc<dyn RtcMembershipPort>,
    pub push: Arc<dyn PushPort>,
    pub room_info: Arc<dyn RoomInfoPort>,
    pub user_info: Arc<dyn UserInfoPort>,
    pub image_packs: Arc<dyn ImagePackPort>,
    pub account: Arc<dyn AccountPort>,
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
    ) -> Result<RestoreOutcome>;
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
    async fn logout(&self) -> DeviceRemoval;
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
pub trait InvitePort: Send + Sync {
    async fn accept(&self, room_id: &RoomId) -> Result<()>;
    async fn decline(&self, room_id: &RoomId) -> Result<()>;
}

#[async_trait]
pub trait LeavePort: Send + Sync {
    async fn leave(&self, room_id: &RoomId) -> Result<()>;
    async fn forget(&self, room_id: &RoomId) -> Result<()>;
    async fn admin_standing(&self, room_id: &RoomId) -> Result<AdminStanding>;
}

#[async_trait]
pub trait UpgradePort: Send + Sync {
    async fn join_successor(&self, room_id: &RoomId) -> Result<()>;
    async fn join_predecessor(&self, room_id: &RoomId) -> Result<()>;
}

#[async_trait]
pub trait RoomPreviewPort: Send + Sync {
    async fn preview(&self, room: &RoomAddress, via: &[String]) -> PreviewResult;
    async fn join(
        &self,
        room: &RoomAddress,
        known: Option<&RoomId>,
        via: &[String],
    ) -> Result<RoomId>;
    async fn fetch_avatars(&self, mxcs: &[String]) -> usize;
}

#[async_trait]
pub trait RoomCreationPort: Send + Sync {
    async fn create_room(&self, blueprint: &RoomBlueprint) -> CreateResult;
}

#[async_trait]
pub trait InvitePeoplePort: Send + Sync {
    async fn search_people(&self, room_id: &RoomId, term: &str) -> SearchResult;
    async fn invite(
        &self,
        room_id: &RoomId,
        user_id: &UserId,
        reason: Option<&str>,
    ) -> InviteResult;
    async fn fetch_avatars(&self, mxcs: &[String]) -> usize;
}

#[async_trait]
pub trait TypingPort: Send + Sync {
    async fn follow_typing(
        &self,
        room_id: &RoomId,
        typists_tx: mpsc::Sender<Vec<NamedUser>>,
    ) -> Result<()>;
    async fn notify_typing(&self, room_id: &RoomId, typing: bool) -> Result<()>;
}

#[async_trait]
pub trait PresencePort: Send + Sync {
    async fn announce(&self, availability: Availability, status: Option<&str>) -> BeaconResult;
    async fn presence_of(&self, user_id: &UserId) -> LookupResult;
}

#[async_trait]
pub trait CallSignalPort: Send + Sync {
    async fn follow_signals(&self, signals_tx: mpsc::Sender<CallSignal>) -> Result<()>;
    async fn decline(&self, room_id: &RoomId, ring: &RingId) -> Result<()>;
    async fn announce_call(&self, room_id: &RoomId, ring: &OutgoingRing) -> Result<RingId>;
    async fn send_key(
        &self,
        room_id: &RoomId,
        key: &OutgoingKey,
        to: &[MemberKey],
    ) -> KeySendReport;
    async fn device_standing(&self, members: &[MemberKey]) -> Vec<(MemberKey, DeviceStanding)>;
}

#[async_trait]
pub trait RtcMembershipPort: Send + Sync {
    async fn own_transports(&self) -> Result<Vec<Transport>>;
    async fn schedule_leave(&self, room_id: &RoomId, delay: Duration) -> ScheduleResult;
    async fn restart_leave(&self, leave: &DelayedLeave) -> RestartResult;
    async fn send_leave(&self, leave: &DelayedLeave) -> Result<()>;
    async fn clear_membership(&self, room_id: &RoomId) -> Result<()>;
    async fn send_membership(
        &self,
        room_id: &RoomId,
        membership: &OwnMembership,
    ) -> Result<SentMembership>;
    async fn sfu_grant(&self, room_id: &RoomId, request: &GrantRequest) -> GrantResult;
}

#[async_trait]
pub trait PushPort: Send + Sync {
    async fn follow_notifications(&self, notices_tx: mpsc::Sender<PushNotice>) -> Result<()>;
}

#[async_trait]
pub trait RoomInfoPort: Send + Sync {
    async fn about(&self, room_id: &RoomId) -> Result<RoomAbout>;
    async fn roster(&self, room_id: &RoomId) -> Result<Vec<RosterMember>>;
    async fn readers(&self, room_id: &RoomId, user_ids: &[String]) -> Result<Vec<Reader>>;
    async fn set_notify(&self, room_id: &RoomId, mode: NotifyMode) -> Result<()>;
    async fn mark_read(&self, room_id: &RoomId) -> Result<()>;
    async fn fetch_avatars(&self, mxcs: &[String]) -> usize;
    async fn room_link(&self, room_id: &RoomId) -> Result<String>;
    async fn event_link(&self, room_id: &RoomId, event_id: &str) -> Result<String>;
}

#[async_trait]
pub trait UserInfoPort: Send + Sync {
    async fn profile(&self, room_id: &RoomId, user_id: &UserId) -> Result<UserProfile>;
    async fn pronouns(&self, user_id: &UserId) -> Vec<String>;
    async fn global_profile(&self, user_id: &UserId) -> Result<GlobalProfile>;
    async fn fetch_avatars(&self, mxcs: &[String]) -> usize;
    async fn start_dm(&self, user_id: &UserId) -> Result<RoomId>;
    async fn set_ignored(&self, user_id: &UserId, change: IgnoreChange) -> Result<()>;
    async fn moderate(
        &self,
        room_id: &RoomId,
        user_id: &UserId,
        action: Moderation,
        reason: Option<&str>,
    ) -> Result<()>;
    async fn follow_trust(&self, user_id: &UserId, trust_tx: mpsc::Sender<UserTrust>)
    -> Result<()>;
}

#[async_trait]
pub trait TimelinePort: Send + Sync {
    async fn subscribe_timeline(
        &self,
        room_id: &RoomId,
        focus: TimelineFocus,
        timeline_tx: mpsc::Sender<TimelineUpdate>,
        cmd_rx: mpsc::UnboundedReceiver<TimelineCommand>,
        close: CancellationToken,
    ) -> Result<()>;
    async fn send_text(&self, room_id: &RoomId, body: &str, emotes: &[Emote]) -> Result<()>;
    async fn send_reply(
        &self,
        room_id: &RoomId,
        body: &str,
        emotes: &[Emote],
        in_reply_to: &str,
    ) -> Result<()>;
    async fn edit_message(&self, room_id: &RoomId, edit: &MessageEdit) -> Result<()>;
    async fn send_poll(&self, room_id: &RoomId, draft: &PollDraft) -> Result<()>;
    async fn send_attachment(
        &self,
        room_id: &RoomId,
        attachment: &OutgoingAttachment,
        abandon: CancellationToken,
    ) -> Result<AttachmentHandoff>;
    async fn resend(&self, room_id: &RoomId, local_id: &str) -> Result<()>;
    async fn discard_send(&self, room_id: &RoomId, local_id: &str) -> Result<()>;
    async fn delete_message(&self, room_id: &RoomId, event_id: &str) -> Result<()>;
}

#[async_trait]
pub trait PinnedPort: Send + Sync {
    async fn subscribe_pinned(
        &self,
        room_id: &RoomId,
        pinned_tx: mpsc::Sender<Vec<PinnedMessage>>,
    ) -> Result<()>;
    async fn change_pin(&self, room_id: &RoomId, event_id: &str, change: PinChange) -> Result<()>;
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
pub struct PackCatalog {
    pub packs: Vec<ImagePack>,
    pub room_encrypted: bool,
}

#[async_trait]
pub trait ImagePackPort: Send + Sync {
    async fn catalog(&self, room_id: &RoomId) -> Result<PackCatalog>;
    async fn prefetch(&self, mxcs: &[String]) -> usize;
    async fn send_sticker(
        &self,
        room_id: &RoomId,
        pack: &PackId,
        shortcode: &str,
        in_reply_to: Option<&str>,
    ) -> Result<()>;
    async fn set_global(&self, pack: &PackId, global: bool) -> Result<()>;
}

#[async_trait]
pub trait VerificationPort: Send + Sync {
    async fn listen_for_verification(
        &self,
        tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Result<()>;
    async fn verify_this_session(&self) -> Result<()>;
    async fn verify_user(&self, user_id: &UserId) -> Result<()>;
    async fn accept_verification(&self) -> Result<()>;
    async fn confirm_verification(&self) -> Result<()>;
    async fn reject_verification(&self) -> Result<()>;
    async fn cancel_verification(&self) -> Result<()>;
}

#[async_trait]
pub trait IdentityPort: Send + Sync {
    async fn follow_room(
        &self,
        room_id: &RoomId,
        alerts_tx: mpsc::Sender<Vec<RoomAlert>>,
    ) -> Result<()>;
    async fn follow_own(&self, own_tx: mpsc::Sender<OwnIdentity>) -> Result<()>;
    async fn pin_identity(&self, user_id: &UserId) -> Result<()>;
    async fn withdraw_verification(&self, user_id: &UserId) -> Result<()>;
    async fn acknowledge_own_reset(&self) -> Result<()>;
}

#[async_trait]
pub trait EncryptionPort: Send + Sync {
    async fn follow(&self, state_tx: mpsc::Sender<AccountEncryption>) -> Result<()>;
    async fn set_up_identity(&self, auth: Option<&AuthAnswer>) -> Result<IdentityOutcome>;
    async fn recover(&self, key: &RecoveryKey) -> Result<RecoveryAttempt>;
    async fn enable_recovery(&self) -> Result<RecoverySetup>;
    async fn change_recovery_key(&self) -> Result<RecoveryKey>;
    async fn reset_identity(&self) -> Result<Guarded>;
    async fn finish_identity_reset(&self, auth: &AuthAnswer) -> Result<Guarded>;
    async fn abandon_identity_reset(&self);
    async fn export_room_keys(&self, path: &Path, passphrase: &KeyPassphrase) -> Result<usize>;
    async fn import_room_keys(&self, path: &Path, passphrase: &KeyPassphrase) -> Result<KeyImport>;
}

#[async_trait]
pub trait AccountPort: Send + Sync {
    async fn facts(&self) -> AccountFacts;
    async fn follow_sessions(&self, sessions_tx: mpsc::Sender<OwnSessions>) -> Result<()>;
    async fn rename_session(&self, device_id: &str, name: &str) -> Result<()>;
    async fn sign_out_session(&self, device_id: &str, auth: Option<&AuthAnswer>)
    -> Result<Guarded>;
    async fn change_password(
        &self,
        password: &AccountPassword,
        sign_out_others: bool,
        auth: Option<&AuthAnswer>,
    ) -> Result<Guarded>;
    async fn deactivate(&self, erase: bool, auth: Option<&AuthAnswer>) -> Result<Guarded>;
    async fn page(&self, page: &AccountPage) -> Option<LauncherSafeUrl>;
    async fn logout_risks(&self) -> LogoutRisks;
}

#[async_trait]
pub trait SessionPort: Send + Sync {
    async fn subscribe_session_changes(
        &self,
        session_tx: mpsc::UnboundedSender<Session>,
    ) -> Result<()>;
    async fn fetch_user_avatar(&self) -> Result<Option<PathBuf>>;
    async fn logout(&self) -> DeviceRemoval;
    async fn suspend(&self);
    async fn clear_store(&self) -> CleanupReport;
}
