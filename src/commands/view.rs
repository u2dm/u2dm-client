use std::path::PathBuf;
use std::time::Duration;
use std::sync::Arc;

use super::messages::{UserMessage, UserMessageKind};
use super::ui::MessageDraft;
use crate::domain::auth::{LoginMethod, Session};
use crate::domain::media::AudioMeta;
use crate::domain::message::PinnedMessage;
use crate::domain::room::{RoomId, RoomList, Space, UnreadFlags};
use crate::domain::space_index::SpaceChild;
use crate::domain::sticker::StickerPacks;
use crate::domain::sync::ConnectionStatus;

#[derive(Clone, Default)]
pub struct AppViewState {
    pub lifecycle: LifecycleView,
    pub connection: ConnectionStatus,
    pub directory: DirectoryView,
    pub space_index: SpaceIndexView,
    pub pagination: PaginationView,
    pub pinned: PinnedView,
    pub stickers: StickerView,
    pub attachment: AttachmentView,
    pub video: VideoView,
    pub audio: AudioView,
    pub unsent: Option<UnsentMessage>,
    pub toast: Toast,
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
    pub draft: MessageDraft,
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
    pub backwards_loading: bool,
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
    Syncing,
    CleaningUp,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct SpaceHeading {
    pub name: String,
    pub member_count: u64,
}

#[derive(Clone)]
pub struct DirectoryView {
    pub rooms: RoomList,
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
}

impl Default for SpaceIndexView {
    fn default() -> Self {
        Self {
            status: SpaceIndexStatus::default(),
            space_name: String::new(),
            rows: Arc::from(Vec::new()),
            avatars_ready: 0,
        }
    }
}
