use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::messages::{UserMessage, UserMessageKind};
use super::ui::Draft;
use crate::domain::auth::{LoginMethod, Session};
use crate::domain::call::MemberKey;
use crate::domain::call::video::{FeedId, ScreenRefusal};
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
use crate::domain::space_index::SpaceChild;
use crate::domain::sync::{ConnectionStatus, ReauthReason};
use crate::domain::timeline::{EventSource, OlderHistory};
use crate::domain::typing::Typists;
use crate::domain::user_info::{UserId, UserProfile};

#[derive(Clone, Default)]
pub struct AppViewState {
    pub lifecycle: LifecycleView,
    pub connection: ConnectionStatus,
    pub directory: DirectoryView,
    pub space_index: SpaceIndexView,
    pub invite: InviteView,
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
    pub desktop: bool,
    pub answerable: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallPhase {
    Joining,
    Ringing,
    Connecting,
    Live,
    Reconnecting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CantDecrypt {
    NotCrossSigned,
    NoKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileState {
    Joining,
    WaitingForKeys,
    Live,
    CantDecrypt(CantDecrypt),
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileKind {
    Person,
    Screen,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicrophoneView {
    On,
    Off,
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
    pub tiles: Arc<[CallTile]>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallView {
    pub ring: Option<RingView>,
    pub here: Option<RoomCallSummary>,
    pub offer: Option<OfferView>,
    pub active: Option<ActiveCall>,
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
    pub space_matches: Arc<[SpaceMatch]>,
    pub spaces: Arc<[Space]>,
    pub subspaces: Arc<[Space]>,
    pub scope: RoomScope,
    pub space_id: String,
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
            space_matches: Arc::from(Vec::new()),
            spaces: Arc::from(Vec::new()),
            subspaces: Arc::from(Vec::new()),
            scope: RoomScope::default(),
            space_id: String::new(),
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
    pub unread: bool,
    pub notify: NotifyMode,
    pub notify_busy: bool,
    pub leaving: bool,
    pub may_invite: bool,
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

#[derive(Clone, Default, PartialEq, Eq)]
pub struct UserInfoView {
    pub card: Option<UserCard>,
    pub direct: DirectChat,
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
