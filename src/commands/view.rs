use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::messages::{UserMessage, UserMessageKind};
use super::ui::Draft;
use crate::domain::auth::{LoginMethod, Session};
use crate::domain::call::MemberKey;
use crate::domain::call::devices::{ByKind, DeviceKey, DeviceKind};
use crate::domain::call::media::MediaChoice;
use crate::domain::call::ring::RingKind;
use crate::domain::call::video::{FeedId, ScreenRefusal};
use crate::domain::capabilities::AccountFacts;
use crate::domain::departure::{AdminStanding, Removal, RemovalList};
use crate::domain::encryption::{KeyStorage, Recovery, RecoveryKey, Standing};
use crate::domain::event_size::Oversize;
use crate::domain::identity::IdentityAlert;
use crate::domain::image_pack::ImagePacks;
use crate::domain::invite::{Invite, InviteList};
use crate::domain::invite_people::Person;
use crate::domain::media::AudioMeta;
use crate::domain::member_name::NamedUser;
use crate::domain::message::{PinnedMessage, TimelineMessage};
use crate::domain::presence::PresenceLine;
use crate::domain::push::PushNotice;
use crate::domain::room::{NotifyMode, RoomId, RoomList, Space, UnreadFlags};
use crate::domain::room_creation::RoomKind;
use crate::domain::room_info::{Reader, RoomAbout, RosterMember};
use crate::domain::room_log::RoomLog;
use crate::domain::room_preview::RoomPreview;
use crate::domain::sessions::{LogoutRisks, OwnSessions};
use crate::domain::space_index::SpaceChild;
use crate::domain::sync::{ConnectionStatus, ReauthReason};
use crate::domain::timeline::{EventSource, OlderHistory};
use crate::domain::typing::Typists;
use crate::domain::user_info::{UserDevice, UserId, UserProfile};

#[derive(Clone, Default)]
pub struct AppViewState {
    pub lifecycle: LifecycleView,
    pub connection: ConnectionStatus,
    pub directory: DirectoryView,
    pub space_index: SpaceIndexView,
    pub invite: InviteView,
    pub removal: RemovalView,
    pub leave_space: LeaveSpaceView,
    pub room_preview: RoomPreviewView,
    pub create_room: CreateRoomView,
    pub invite_people: InvitePeopleView,
    pub room_info: RoomInfoView,
    pub room_menu: Option<RoomMenuTarget>,
    pub user_info: UserInfoView,
    pub pagination: PaginationView,
    pub pinned: PinnedView,
    pub image_packs: PackView,
    pub attachment: AttachmentView,
    pub video: VideoView,
    pub audio: AudioView,
    pub unsent: Option<UnsentMessage>,
    pub identity_alert: IdentityAlertView,
    pub upgrade: UpgradeView,
    pub typing: TypingView,
    pub presence: PresenceView,
    pub call: CallView,
    pub notifications: NotificationsView,
    pub encryption: EncryptionView,
    pub account: AccountView,
    pub logout: LogoutView,
    pub toast: Toast,
    pub message_link: CopiedLink,
    pub room_link: CopiedLink,
    pub source: SourceState,
    pub readers: ReadersView,
    pub mentions: MentionsView,
    pub room_log: Option<RoomLogView>,
}

impl AppViewState {
    pub fn logged_out() -> Self {
        Self {
            lifecycle: LifecycleView {
                step: LoginStep::Homeserver,
                ..LifecycleView::default()
            },
            ..Self::default()
        }
    }

    pub fn signing_out() -> Self {
        Self {
            lifecycle: LifecycleView {
                step: LoginStep::Homeserver,
                activity: LoginActivity::SigningOut,
                ..LifecycleView::default()
            },
            ..Self::default()
        }
    }

    pub fn reauthenticating(session: &Session, reason: ReauthReason) -> Self {
        Self {
            lifecycle: LifecycleView {
                step: LoginStep::Reauthenticate(reason),
                method: session.login_method(),
                resolved_homeserver: session.homeserver.clone(),
                user_id: session.user_id.clone(),
                ..LifecycleView::default()
            },
            ..Self::default()
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub enum SourceState {
    #[default]
    Closed,
    Locating {
        event_id: String,
    },
    Ready(Arc<EventSource>),
    Unavailable {
        event_id: String,
    },
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum ReadersStatus {
    #[default]
    Closed,
    Locating,
    Ready,
    Unavailable,
}

#[derive(Clone, Default)]
pub struct ReadersView {
    pub status: ReadersStatus,
    pub message: Option<Arc<TimelineMessage>>,
    pub total: usize,
    pub rows: Arc<[Arc<Reader>]>,
    pub has_more: bool,
    pub pages_landed: i32,
    pub avatars_ready: usize,
}

#[derive(Clone, Default)]
pub struct MentionsView {
    pub room_id: Option<RoomId>,
    pub offers_room: bool,
    pub rows: Arc<[Arc<RosterMember>]>,
    pub avatars_ready: usize,
}

#[derive(Clone, PartialEq, Eq)]
pub struct RoomLogView {
    pub room_id: RoomId,
    pub name: String,
    pub log: RoomLog,
    pub lines_landed: i32,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct CopiedLink {
    pub serial: i32,
    pub url: String,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct AttachmentView {
    pub pick: u64,
    pub visible: bool,
    pub filename: String,
    pub mimetype: String,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    pub kind: AttachmentKind,
    pub duration: Option<Duration>,
    pub preview_path: Option<PathBuf>,
    pub sending: bool,
    pub error: UserMessageKind,
    pub error_detail: String,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct VideoView {
    pub visible: bool,
    pub loading: bool,
    pub path: Option<PathBuf>,
    pub error: UserMessageKind,
}

#[derive(Clone, PartialEq, Eq)]
pub struct UnsentMessage {
    pub submission: i32,
    pub room_id: RoomId,
    pub draft: Draft,
    pub oversize: Option<Oversize>,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct IdentityAlertView {
    pub alert: Option<IdentityAlert>,
    pub busy: bool,
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub enum UpgradeLink {
    #[default]
    Hidden,
    Open(RoomId),
    Join(RoomId),
    Joining(RoomId),
}

impl UpgradeLink {
    pub fn room_id(&self) -> Option<&RoomId> {
        match self {
            Self::Hidden => None,
            Self::Open(room_id) | Self::Join(room_id) | Self::Joining(room_id) => Some(room_id),
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct UpgradeView {
    pub room_id: Option<RoomId>,
    pub successor: UpgradeLink,
    pub predecessor: UpgradeLink,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct TypingView {
    pub room_id: Option<RoomId>,
    pub typists: Typists,
}

#[derive(Clone, PartialEq, Eq)]
pub struct PartnerPresence {
    pub room_id: RoomId,
    pub user_id: UserId,
    pub line: PresenceLine,
}

#[derive(Clone, PartialEq, Eq)]
pub struct PersonPresence {
    pub user_id: UserId,
    pub line: PresenceLine,
    pub status: Option<String>,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct PresenceView {
    pub partner: Option<PartnerPresence>,
    pub person: Option<PersonPresence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct RingView {
    pub serial: u64,
    pub room_id: RoomId,
    pub room_name: String,
    pub room_avatar_mxc: Option<String>,
    pub direct: bool,
    pub caller: NamedUser,
    pub video: bool,
    pub quiet: bool,
    pub desktop: bool,
    pub answerable: bool,
    pub refusal: Option<JoinRefusal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomCallSummary {
    pub room_id: RoomId,
    pub people: usize,
    pub video: bool,
    pub joined_by_you: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinRefusal {
    NotBuilt,
    MediaBroken,
    NoServer,
    NotAllowed,
    SessionUnverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinOffer {
    Start,
    Join,
    InCall,
    Refused(JoinRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferView {
    pub room_id: RoomId,
    pub offer: JoinOffer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeavingCall {
    pub room_id: RoomId,
    pub room_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSheetView {
    pub serial: i32,
    pub room_id: RoomId,
    pub room_name: String,
    pub direct: bool,
    pub offer: JoinOffer,
    pub video: bool,
    pub people: usize,
    pub others: Arc<[NamedUser]>,
    pub devices: Option<CallDevices>,
    pub proposed: MediaChoice,
    pub preview: Option<SheetPreview>,
    pub leaving: Option<LeavingCall>,
    pub ended: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SheetPreview {
    Starting,
    Local(FeedId),
    InCall { feed: FeedId, room_name: String },
    Elsewhere { room_name: String },
    Failed(PreviewTrouble),
}

impl SheetPreview {
    pub const fn feed(&self) -> Option<&FeedId> {
        match self {
            Self::Local(feed) | Self::InCall { feed, .. } => Some(feed),
            Self::Starting | Self::Elsewhere { .. } | Self::Failed(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewTrouble {
    NoCamera,
    NotStarted,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallPhase {
    Joining,
    Calling,
    Connecting,
    Waiting,
    Live,
    Reconnecting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallNotice {
    NotAnnounced(RingKind),
    MicrophoneFailed,
    CameraFailed,
    ScreenFailed,
    ChoosingScreen,
    SharingNeverStarted,
    ConnectionPoor,
    CantDecrypt {
        who: NamedUser,
        why: CantDecrypt,
        others: usize,
    },
    NotReceived {
        who: NamedUser,
        what: MissingMedia,
        others: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallFailure {
    ServerUnreachable,
    JoinFailed,
    MediaFailed,
    CutOff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedCall {
    pub room_id: RoomId,
    pub room_name: String,
    pub failure: CallFailure,
    pub retry: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CantDecrypt {
    NotCrossSigned,
    NoKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingMedia {
    Audio,
    Video,
    AudioAndVideo,
    Screen,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileState {
    Joining,
    WaitingForKeys,
    Live,
    PoorConnection,
    NotReceived(MissingMedia),
    ConnectionLost,
    CantDecrypt(CantDecrypt),
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TileKind {
    Person,
    Screen,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub member: MemberKey,
    pub kind: TileKind,
}

impl fmt::Display for TileKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            TileKind::Person => "person",
            TileKind::Screen => "screen",
        };
        write!(f, "{} {kind}", self.member)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallTile {
    pub member: MemberKey,
    pub name: NamedUser,
    pub own: bool,
    pub kind: TileKind,
    pub state: TileState,
    pub muted: bool,
    pub speaking: bool,
    pub video: Option<FeedId>,
}

impl CallTile {
    pub fn key(&self) -> TileKey {
        TileKey {
            member: self.member.clone(),
            kind: self.kind,
        }
    }

    pub const fn is_self_view(&self) -> bool {
        self.own && matches!(self.kind, TileKind::Person)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareNews {
    pub serial: u64,
    pub who: NamedUser,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallPerson {
    pub user: NamedUser,
    pub you: bool,
    pub devices: usize,
    pub muted: bool,
    pub sharing: bool,
    pub state: TileState,
    pub tile: Option<TileKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStatus {
    SystemDefault,
    Connected,
    Missing,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceOption {
    pub key: Option<DeviceKey>,
    pub name: String,
    pub status: DeviceStatus,
    pub chosen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceTrouble {
    Disconnected,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceNews {
    pub serial: u64,
    pub kind: DeviceKind,
    pub name: String,
    pub trouble: DeviceTrouble,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallDevices {
    pub options: ByKind<Arc<[DeviceOption]>>,
    pub news: Option<DeviceNews>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicrophoneView {
    On,
    Off,
    Starting,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraView {
    Off,
    Starting,
    On,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenView {
    Off,
    Choosing,
    On,
    Failed,
    Unavailable(ScreenRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct ActiveCall {
    pub call: u64,
    pub room_id: RoomId,
    pub room_name: String,
    pub phase: CallPhase,
    pub live_since_ms: Option<u64>,
    pub encrypted: bool,
    pub video: bool,
    pub microphone: MicrophoneView,
    pub camera: CameraView,
    pub screen: ScreenView,
    pub notice: Option<CallNotice>,
    pub stage: Option<CallTile>,
    pub stage_chosen: bool,
    pub tiles: Arc<[CallTile]>,
    pub self_view: bool,
    pub shares: Arc<[NamedUser]>,
    pub offered_share: Option<NamedUser>,
    pub started_share: Option<ShareNews>,
    pub ended_share: Option<ShareNews>,
    pub others: Arc<[NamedUser]>,
    pub people: Arc<[CallPerson]>,
    pub devices: CallDevices,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallView {
    pub ring: Option<RingView>,
    pub here: Option<RoomCallSummary>,
    pub offer: Option<OfferView>,
    pub sheet: Option<CallSheetView>,
    pub active: Option<ActiveCall>,
    pub failed: Option<FailedCall>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownNotice {
    pub serial: u64,
    pub notice: PushNotice,
}

#[derive(Clone, PartialEq, Eq)]
pub struct NotificationsView {
    pub shown: Arc<[ShownNotice]>,
}

impl Default for NotificationsView {
    fn default() -> Self {
        Self {
            shown: Arc::from(Vec::new()),
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct EncryptionView {
    pub standing: Standing,
    pub verified_sessions: bool,
    pub recovery: Recovery,
    pub key_storage: KeyStorage,
    pub dialog: Option<EncryptionDialog>,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct EncryptionDialog {
    pub step: EncryptionStep,
    pub activity: EncryptionActivity,
    pub error: UserMessage,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub enum EncryptionStep {
    #[default]
    Overview,
    EnteringKey,
    Unlocked,
    UnlockedWithoutBackup,
    ShowingKey(RecoveryKey),
    ConfirmingReset,
    AuthenticatingSetup(AuthPrompt),
    AuthenticatingReset(AuthPrompt),
    IdentityReset,
    ExportingKeys(String),
    KeysExported {
        count: usize,
        file: String,
    },
    ImportingKeys(String),
    KeysImported {
        imported: usize,
        total: usize,
    },
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum EncryptionActivity {
    #[default]
    None,
    Requesting,
    Unlocking,
    SettingUpRecovery,
    ChangingKey,
    RetryingSetup,
    ResettingIdentity,
    Authenticating,
    ChoosingFile,
    ExportingKeys,
    ImportingKeys,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum AuthStage {
    #[default]
    Password,
    Approve,
    Browser,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct AuthPrompt {
    pub stage: AuthStage,
    pub error: UserMessage,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct AccountView {
    pub dialog: Option<AccountDialog>,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct AccountDialog {
    pub step: AccountStep,
    pub activity: AccountActivity,
    pub busy_session: Option<String>,
    pub sessions: Option<OwnSessions>,
    pub facts: Option<AccountFacts>,
    pub error: UserMessage,
    pub notice: UserMessage,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub enum AccountStep {
    #[default]
    Overview,
    ChangingPassword,
    ConfirmingDeactivation,
    Authenticating(AccountAuth),
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct AccountAuth {
    pub prompt: AuthPrompt,
    pub guarding: AccountActivity,
    pub subject: String,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum AccountActivity {
    #[default]
    None,
    Renaming,
    SigningOut,
    ChangingPassword,
    Deactivating,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct LogoutView {
    pub warning: Option<LogoutRisks>,
}

#[derive(Clone, Default, PartialEq)]
pub struct AudioView {
    pub now_playing: Option<NowPlaying>,
}

#[derive(Clone, PartialEq)]
pub struct NowPlaying {
    pub request: u64,
    pub room_id: RoomId,
    pub event_id: String,
    pub sender: String,
    pub meta: AudioMeta,
    pub file: TrackFile,
}

#[derive(Clone, PartialEq, Eq)]
pub enum TrackFile {
    Downloading,
    Ready(PathBuf),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum RoomScope {
    #[default]
    All,
    Direct,
    Space,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum InvitesSection {
    #[default]
    Folded,
    Unfolded,
    Matches,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum AttachmentKind {
    #[default]
    File,
    Image,
    Video,
    Audio,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub enum Toast {
    #[default]
    None,
    Error(UserMessage),
    FileSaved(String),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct PaginationView {
    pub generation: i32,
    pub older_history: OlderHistory,
    pub forwards_loading: bool,
    pub new_messages: u32,
}

impl PaginationView {
    pub fn retarget(&mut self, generation: i32) {
        if self.generation != generation {
            *self = Self {
                generation,
                ..Self::default()
            };
        }
    }
}

#[derive(Clone, Default, PartialEq)]
pub struct PinnedView {
    pub room_id: Option<RoomId>,
    pub messages: Arc<[PinnedMessage]>,
    pub shown: usize,
    pub pinned_ids: Arc<BTreeSet<String>>,
}

impl PinnedView {
    pub fn shown_in(&self, room_id: &str) -> Option<&PinnedMessage> {
        if self.room_id.as_deref() != Some(room_id) {
            return None;
        }
        self.messages.get(self.shown)
    }
}

#[derive(Clone)]
pub struct PackView {
    pub generation: i32,
    pub packs: ImagePacks,
    pub ready_images: usize,
    pub room_encrypted: bool,
    pub loading: bool,
}

impl Default for PackView {
    fn default() -> Self {
        Self {
            generation: 0,
            packs: Arc::from(Vec::new()),
            ready_images: 0,
            room_encrypted: false,
            loading: false,
        }
    }
}

#[derive(Clone, Default)]
pub struct LifecycleView {
    pub step: LoginStep,
    pub activity: LoginActivity,
    pub messages: Vec<UserMessage>,
    pub method: LoginMethod,
    pub resolved_homeserver: String,
    pub user_id: String,
    pub device_id: String,
    pub avatar_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum LoginStep {
    #[default]
    Loading,
    RestorePaused,
    Homeserver,
    Credentials,
    Reauthenticate(ReauthReason),
    LoggedIn,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum LoginActivity {
    #[default]
    Idle,
    LoadingSession,
    OpeningStore,
    Connecting,
    RestoringAuth,
    CheckingServer,
    LoggingIn,
    OpeningBrowser,
    WaitingAuth,
    Cancelling,
    Syncing,
    SigningOut,
    CleaningUp,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct SpaceHeading {
    pub id: String,
    pub name: String,
    pub member_count: u64,
    pub may_invite: bool,
    pub may_add_children: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SpaceMatch {
    pub id: String,
    pub name: String,
    pub avatar_mxc: Option<String>,
    pub parent: Option<String>,
    pub rooms: usize,
    pub flags: UnreadFlags,
}

#[derive(Clone)]
pub struct DirectoryView {
    pub rooms: RoomList,
    pub invites: InviteList,
    pub invites_section: InvitesSection,
    pub removals: RemovalList,
    pub space_matches: Arc<[SpaceMatch]>,
    pub spaces: Arc<[Space]>,
    pub subspaces: Arc<[Space]>,
    pub scope: RoomScope,
    pub space_id: String,
    pub space_name: String,
    pub subspace_id: String,
    pub listed_space: SpaceHeading,
    pub direct_flags: UnreadFlags,
    pub invites_pending: bool,
    pub attention: usize,
}

impl Default for DirectoryView {
    fn default() -> Self {
        Self {
            rooms: Arc::from(Vec::new()),
            invites: Arc::from(Vec::new()),
            invites_section: InvitesSection::default(),
            removals: Arc::from(Vec::new()),
            space_matches: Arc::from(Vec::new()),
            spaces: Arc::from(Vec::new()),
            subspaces: Arc::from(Vec::new()),
            scope: RoomScope::default(),
            space_id: String::new(),
            space_name: String::new(),
            subspace_id: String::new(),
            listed_space: SpaceHeading::default(),
            direct_flags: UnreadFlags::default(),
            invites_pending: false,
            attention: 0,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum CreateRoomStatus {
    #[default]
    Closed,
    Editing,
    Creating,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct CreateRoomView {
    pub status: CreateRoomStatus,
    pub kind: RoomKind,
    pub parent_name: String,
    pub server_name: String,
    pub error: UserMessage,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum PeopleSearchStatus {
    #[default]
    Idle,
    Searching,
    Ready,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InviteeState {
    Available,
    Picked,
    Joined,
    Invited,
    Banned,
    Sending,
    Failed,
}

#[derive(Clone, PartialEq, Eq)]
pub struct InviteeRow {
    pub person: Arc<Person>,
    pub state: InviteeState,
    pub failure: UserMessage,
}

#[derive(Clone, PartialEq, Eq)]
pub struct InviteTarget {
    pub room_id: RoomId,
    pub name: String,
    pub space: bool,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct InvitePeopleView {
    pub target: Option<InviteTarget>,
    pub search: PeopleSearchStatus,
    pub results: Arc<[InviteeRow]>,
    pub picked: Arc<[InviteeRow]>,
    pub limited: bool,
    pub sending: bool,
    pub error: UserMessage,
    pub avatars_ready: usize,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum PendingAnswer {
    #[default]
    None,
    Accept,
    Decline,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct InviteView {
    pub invite: Option<Arc<Invite>>,
    pub answer: PendingAnswer,
    pub error: UserMessage,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct RemovalView {
    pub removal: Option<Arc<Removal>>,
    pub forgetting: bool,
    pub error: UserMessage,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SpaceToLeave {
    pub id: RoomId,
    pub name: String,
    pub rooms: usize,
    pub spaces: usize,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct LeaveSpaceView {
    pub space: Option<SpaceToLeave>,
    pub standing: Option<AdminStanding>,
    pub leaving: bool,
    pub error: UserMessage,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub enum RoomPreviewSheet {
    #[default]
    Closed,
    Idle,
    Loading,
    Loaded(ShownPreview),
    Missing,
    Unavailable,
    Failed,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ShownPreview {
    pub room: Arc<RoomPreview>,
    pub opens_event: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct RoomPreviewView {
    pub sheet: RoomPreviewSheet,
    pub address: String,
    pub entry: bool,
    pub access: ChildAccess,
    pub error: UserMessage,
    pub avatars_ready: usize,
}

impl Default for RoomPreviewView {
    fn default() -> Self {
        Self {
            sheet: RoomPreviewSheet::default(),
            address: String::new(),
            entry: false,
            access: ChildAccess::Join,
            error: UserMessage::default(),
            avatars_ready: 0,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SpaceIndexStatus {
    #[default]
    Closed,
    Loading,
    Partial,
    Complete,
    LoadingMore,
    Failed,
    MoreFailed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChildAccess {
    Open,
    OpenSubspace,
    Joined,
    Joining,
    Join,
    InviteOnly,
    Knock,
    MembersOnly,
    Unavailable,
}

#[derive(Clone, PartialEq)]
pub struct SpaceIndexRow {
    pub child: Arc<SpaceChild>,
    pub access: ChildAccess,
}

#[derive(Clone)]
pub struct SpaceIndexView {
    pub status: SpaceIndexStatus,
    pub space_name: String,
    pub rows: Arc<[SpaceIndexRow]>,
    pub avatars_ready: usize,
    pub pages_landed: i32,
}

impl Default for SpaceIndexView {
    fn default() -> Self {
        Self {
            status: SpaceIndexStatus::default(),
            space_name: String::new(),
            rows: Arc::from(Vec::new()),
            avatars_ready: 0,
            pages_landed: 0,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RoomCard {
    pub id: RoomId,
    pub name: String,
    pub avatar_mxc: Option<String>,
    pub member_count: u64,
    pub is_direct: bool,
    pub topic: Option<String>,
    pub alias: Option<String>,
    pub notify: NotifyMode,
    pub may_invite: bool,
    pub readable_by_anyone: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum RosterStatus {
    #[default]
    Loading,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum RoomInfoPlacement {
    #[default]
    Closed,
    Dialog,
    Pane,
}

#[derive(Clone, PartialEq, Eq)]
pub enum RosterRow {
    Member(Arc<RosterMember>),
    InvitedHeading,
}

#[derive(Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct RoomMenuTarget {
    pub room_id: RoomId,
    pub name: String,
    pub is_space: bool,
    pub unread: bool,
    pub notify: NotifyMode,
    pub notify_busy: bool,
    pub leaving: bool,
    pub may_invite: bool,
    pub may_add_children: bool,
    pub has_call: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CardStatus {
    Loaded,
    ReadFailed,
    Retrying,
}

#[derive(Clone, PartialEq, Eq)]
pub struct UserCard {
    pub room_id: RoomId,
    pub profile: UserProfile,
    pub status: CardStatus,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum DirectChat {
    #[default]
    Hidden,
    Open,
    Start,
    Starting,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum PendingModeration {
    #[default]
    None,
    Kick,
    Ban,
    Unban,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum VerifyOffer {
    #[default]
    Hidden,
    Offered,
    Requesting,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct UserInfoView {
    pub card: Option<UserCard>,
    pub direct: DirectChat,
    pub verify: VerifyOffer,
    pub sessions: Option<Arc<[UserDevice]>>,
    pub ignore_busy: bool,
    pub moderating: PendingModeration,
    pub avatars_ready: usize,
    pub error: UserMessage,
}

#[derive(Clone, Default)]
pub struct RoomInfoView {
    pub placement: RoomInfoPlacement,
    pub card: Option<RoomCard>,
    pub about: Option<RoomAbout>,
    pub roster: RosterStatus,
    pub rows: Arc<[RosterRow]>,
    pub has_more: bool,
    pub pages_landed: i32,
    pub avatars_ready: usize,
    pub notify: NotifyMode,
    pub notify_busy: bool,
    pub leaving: bool,
    pub error: UserMessage,
}
