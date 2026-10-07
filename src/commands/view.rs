use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;
use std::sync::Arc;

use super::messages::{UserMessage, UserMessageKind};
use super::ui::Draft;
use crate::domain::auth::{LoginMethod, Session};
use crate::domain::media::AudioMeta;
use crate::domain::message::{PinnedMessage, TimelineMessage};
use crate::domain::room::{NotifyMode, RoomId, RoomList, Space, UnreadFlags};
use crate::domain::room_info::{Reader, RoomAbout, RosterMember};
use crate::domain::room_log::RoomLog;
use crate::domain::space_index::SpaceChild;
use crate::domain::sticker::StickerPacks;
use crate::domain::sync::ConnectionStatus;
use crate::domain::timeline::{EventSource, OlderHistory};
use crate::domain::user_info::UserProfile;

#[derive(Clone, Default)]
pub struct AppViewState {
    pub lifecycle: LifecycleView,
    pub connection: ConnectionStatus,
    pub directory: DirectoryView,
    pub space_index: SpaceIndexView,
    pub room_info: RoomInfoView,
    pub room_menu: Option<RoomMenuTarget>,
    pub user_info: UserInfoView,
    pub pagination: PaginationView,
    pub pinned: PinnedView,
    pub stickers: StickerView,
    pub attachment: AttachmentView,
    pub video: VideoView,
    pub audio: AudioView,
    pub unsent: Option<UnsentMessage>,
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

    pub fn reauthenticating(session: &Session) -> Self {
        Self {
            lifecycle: LifecycleView {
                step: LoginStep::Reauthenticate,
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
pub struct StickerView {
    pub generation: i32,
    pub packs: StickerPacks,
    pub ready_images: usize,
    pub room_encrypted: bool,
    pub loading: bool,
}

impl Default for StickerView {
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
    pub avatar_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum LoginStep {
    #[default]
    Loading,
    RestorePaused,
    Homeserver,
    Credentials,
    Reauthenticate,
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
    CleaningUp,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct SpaceHeading {
    pub name: String,
    pub member_count: u64,
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
    pub space_matches: Arc<[SpaceMatch]>,
    pub spaces: Arc<[Space]>,
    pub subspaces: Arc<[Space]>,
    pub scope: RoomScope,
    pub space_id: String,
    pub subspace_id: String,
    pub listed_space: SpaceHeading,
    pub direct_flags: UnreadFlags,
}

impl Default for DirectoryView {
    fn default() -> Self {
        Self {
            rooms: Arc::from(Vec::new()),
            space_matches: Arc::from(Vec::new()),
            spaces: Arc::from(Vec::new()),
            subspaces: Arc::from(Vec::new()),
            scope: RoomScope::default(),
            space_id: String::new(),
            subspace_id: String::new(),
            listed_space: SpaceHeading::default(),
            direct_flags: UnreadFlags::default(),
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
pub struct RoomMenuTarget {
    pub room_id: RoomId,
    pub name: String,
    pub unread: bool,
    pub notify: NotifyMode,
    pub notify_busy: bool,
    pub leaving: bool,
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
