mod call;
mod rooms;
mod security;
mod settings;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub(crate) use self::call::{
    ActiveCall, CallDevices, CallFace, CallFailure, CallNotice, CallPerson, CallPhase,
    CallSheetView, CallTile, CallView, CameraView, CantDecrypt, DeviceNews, DeviceOption,
    DeviceStatus, DeviceTestView, DeviceTrouble, FailedCall, HandNews, HandView, JoinOffer,
    JoinRefusal, LeavingCall, LinkView, MicrophoneView, MissingMedia, OfferView, PreviewTrouble,
    ReactionNews, ReactionView, RingView, RoomCallSummary, ScreenView, ServerDetails, ShareNews,
    SheetPreview, SheetTests, TestPhase, TileKey, TileKind, TileState,
};
pub(crate) use self::rooms::{
    CardStatus, ChildAccess, CreateRoomStatus, CreateRoomView, DirectChat, DirectoryView,
    InvitePeopleView, InviteTarget, InviteView, InviteeRow, InviteeState, InvitesSection,
    LeaveSpaceView, PendingAnswer, PendingModeration, PeopleSearchStatus, RemovalView, RoomCard,
    RoomInfoPlacement, RoomInfoView, RoomMenuTarget, RoomPreviewSheet, RoomPreviewView, RoomScope,
    RosterRow, RosterStatus, ShownPreview, SpaceHeading, SpaceIndexRow, SpaceIndexStatus,
    SpaceIndexView, SpaceMatch, SpaceToLeave, UserCard, UserInfoView, VerifyOffer,
};
pub(crate) use self::security::{
    AccountActivity, AccountAuth, AccountDialog, AccountStep, AccountView, AuthPrompt, AuthStage,
    EncryptionActivity, EncryptionDialog, EncryptionStep, EncryptionView, LifecycleView,
    LoginActivity, LoginStep, LogoutView,
};
pub(crate) use self::settings::{
    AccountRule, DownloadedMedia, IgnoredPerson, IgnoredUsersView, KeywordRow,
    NotificationRulesView, SettingsDialog, SettingsPage, SettingsView,
};
use super::messages::{UserMessage, UserMessageKind};
use super::ui::Draft;
use crate::domain::auth::Session;
use crate::domain::event_size::Oversize;
use crate::domain::identity::IdentityAlert;
use crate::domain::image_pack::ImagePacks;
use crate::domain::media::AudioMeta;
use crate::domain::message::{PinnedMessage, TimelineMessage};
use crate::domain::presence::PresenceLine;
use crate::domain::push::PushNotice;
use crate::domain::room::RoomId;
use crate::domain::room_info::{Reader, RosterMember};
use crate::domain::room_log::RoomLog;
use crate::domain::settings::Settings;
use crate::domain::sync::{ConnectionStatus, ReauthReason};
use crate::domain::timeline::{EventSource, OlderHistory};
use crate::domain::typing::Typists;
use crate::domain::user_info::UserId;

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
    pub settings: SettingsView,
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
    pub fn seeded(values: &Settings) -> Self {
        Self {
            settings: SettingsView {
                values: values.clone(),
                ..SettingsView::default()
            },
            ..Self::default()
        }
    }

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
pub struct ShownNotice {
    pub serial: u64,
    pub notice: PushNotice,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct NotificationsView {
    pub shown: Arc<[ShownNotice]>,
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
