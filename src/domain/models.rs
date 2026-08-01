use std::sync::Arc;
use std::{fmt, ops};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethod {
    Password,
    OAuth,
}

impl AuthMethod {
    pub fn from_login_type(login_type: &str) -> Option<Self> {
        match login_type {
            "m.login.password" => Some(Self::Password),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoginMethod {
    Password,
    OAuth,
    Both,
    #[default]
    None,
}

impl LoginMethod {
    pub fn from_auth_methods(methods: &[AuthMethod]) -> Self {
        match (
            methods.contains(&AuthMethod::Password),
            methods.contains(&AuthMethod::OAuth),
        ) {
            (true, true) => Self::Both,
            (true, false) => Self::Password,
            (false, true) => Self::OAuth,
            (false, false) => Self::None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub auth_methods: Vec<AuthMethod>,
    pub unsupported_flows: Vec<String>,
    pub homeserver_url: String,
}

#[derive(Debug, Clone)]
pub struct LoginCredentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone)]
pub struct OAuthLoginData {
    pub auth_url: String,
}

#[derive(Clone)]
pub struct Session {
    pub user_id: String,
    pub device_id: String,
    pub homeserver: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub client_id: Option<String>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("user_id", &self.user_id)
            .field("device_id", &self.device_id)
            .field("homeserver", &self.homeserver)
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("client_id", &self.client_id)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomId(String);

impl RoomId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl ops::Deref for RoomId {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for RoomId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RoomId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessagePreviewKind {
    #[default]
    None,
    Text,
    Image,
    Video,
    Audio,
    File,
    Location,
    Encrypted,
    Sticker,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NotifyMode {
    #[default]
    AllMessages,
    MentionsOnly,
    Muted,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Room {
    pub id: RoomId,
    pub display_name: String,
    pub avatar_mxc: Option<String>,
    pub is_direct: bool,
    pub member_count: u64,
    pub has_unread: bool,
    pub has_mentions: bool,
    pub has_activity: bool,
    pub notify: NotifyMode,
    pub last_activity_ts: u64,
    pub last_message_sender: Option<String>,
    pub last_message_kind: MessagePreviewKind,
    pub last_message_body: String,
    pub last_message_service: Option<ServiceEvent>,
    pub last_message_is_own: bool,
    pub last_message_edited: bool,
}

impl Room {
    pub fn muted(&self) -> bool {
        matches!(self.notify, NotifyMode::Muted)
    }

    pub fn alert(&self) -> bool {
        !self.muted() && (self.has_unread || self.has_mentions)
    }

    pub fn mention(&self) -> bool {
        self.has_mentions && !self.muted()
    }

    pub fn hint(&self) -> bool {
        self.has_activity && !self.alert()
    }
}

pub type RoomList = Arc<[Arc<Room>]>;

#[derive(Debug, Clone, PartialEq)]
pub struct Space {
    pub id: String,
    pub name: String,
    pub avatar_mxc: Option<String>,
    pub child_room_ids: Vec<String>,
    pub child_space_ids: Vec<String>,
    pub order: Option<String>,
    pub alert: bool,
    pub mention: bool,
    pub hint: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ConnectionStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

#[derive(Debug)]
pub enum SyncEvent {
    Connected,
    Rooms(RoomList),
    Spaces(Arc<[Space]>),
    ConnectionError(String),
}

#[derive(Debug)]
pub enum SyncOutcome {
    Cancelled,
    Recoverable(String),
    SessionExpired,
    Fatal(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageMeta {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub mimetype: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileMeta {
    pub filename: String,
    pub mimetype: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceEvent {
    Joined,
    Left,
    Invited { target: Option<String> },
    InvitationAccepted,
    InvitationRejected,
    InvitationRevoked { target: Option<String> },
    Kicked { target: Option<String> },
    Banned { target: Option<String> },
    Unbanned { target: Option<String> },
    Knocked,
    KnockAccepted { target: Option<String> },
    DisplayNameSet { name: String },
    DisplayNameChanged { name: String },
    DisplayNameRemoved,
    AvatarChanged,
    RoomNameChanged { name: String },
    RoomTopicChanged,
    RoomAvatarChanged,
    RoomCreated,
    EncryptionEnabled,
    CallStarted,
    CallNotification,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageBody {
    Text(String),
    Notice(String),
    Emote(String),
    Image {
        caption: Option<String>,
        meta: ImageMeta,
    },
    File {
        meta: FileMeta,
    },
    Service(ServiceEvent),
    UnableToDecrypt,
    Unsupported {
        kind: String,
        fallback: String,
    },
}

impl MessageBody {
    pub fn service(&self) -> Option<&ServiceEvent> {
        match self {
            Self::Service(event) => Some(event),
            _ => None,
        }
    }

    pub fn preview_kind(&self) -> MessagePreviewKind {
        match self {
            Self::Text(_)
            | Self::Notice(_)
            | Self::Emote(_)
            | Self::Service(_)
            | Self::Unsupported { .. } => MessagePreviewKind::Text,
            Self::Image { .. } => MessagePreviewKind::Image,
            Self::File { .. } => MessagePreviewKind::File,
            Self::UnableToDecrypt => MessagePreviewKind::Encrypted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyInfo {
    pub event_id: String,
    pub sender: String,
    pub kind: MessagePreviewKind,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineMessage {
    pub unique_id: String,
    pub event_id: Option<String>,
    pub sender: String,
    pub sender_display_name: Option<String>,
    pub sender_avatar_url: Option<String>,
    pub sender_pronouns: Vec<String>,
    pub body: MessageBody,
    pub timestamp: u64,
    pub is_own: bool,
    pub reply: Option<ReplyInfo>,
    pub edited: bool,
    pub is_first_unread: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnreadAnchor {
    pub row: usize,
    pub count: u32,
}

#[derive(Debug, Clone, strum::IntoStaticStr)]
pub enum TimelinePatch {
    Reset(Vec<TimelineMessage>),
    Append(Vec<TimelineMessage>),
    PushFront(TimelineMessage),
    PushBack(TimelineMessage),
    Insert {
        index: usize,
        message: TimelineMessage,
    },
    Set {
        index: usize,
        message: TimelineMessage,
    },
    Remove {
        index: usize,
    },
    PopFront,
    PopBack,
    Truncate {
        length: usize,
    },
    Clear,
    Batch(Vec<TimelinePatch>),
    Enrich(EnrichmentDelta),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailOutcome {
    Unchanged,
    Ready,
    Failed,
}

#[derive(Debug, Clone)]
pub struct EnrichmentDelta {
    pub unique_id: String,
    pub event_id: Option<String>,
    pub thumbnail: ThumbnailOutcome,
    pub avatar_mxc: Option<String>,
    pub pronouns: Option<Vec<String>>,
}

impl EnrichmentDelta {
    pub fn is_noop(&self) -> bool {
        matches!(self.thumbnail, ThumbnailOutcome::Unchanged)
            && self.avatar_mxc.is_none()
            && self.pronouns.is_none()
    }
}

impl TimelinePatch {
    pub fn label(&self) -> &'static str {
        self.into()
    }

    pub fn is_prepend(&self) -> bool {
        self.adds_at_front() && !self.adds_at_back()
    }

    pub fn opens_room(&self) -> bool {
        self.last_reset().is_some()
    }

    pub fn unread_anchor(&self) -> Option<UnreadAnchor> {
        let messages = self.last_reset()?;
        let row = messages.iter().position(|m| m.is_first_unread)?;
        Some(UnreadAnchor {
            row,
            count: u32::try_from(messages.len() - row).unwrap_or(u32::MAX),
        })
    }

    pub fn shifts_rows(&self) -> bool {
        match self {
            Self::PushFront(_)
            | Self::PopFront
            | Self::Insert { .. }
            | Self::Remove { .. }
            | Self::Truncate { .. } => true,
            Self::Batch(patches) => patches.iter().any(TimelinePatch::shifts_rows),
            _ => false,
        }
    }

    fn last_reset(&self) -> Option<&[TimelineMessage]> {
        match self {
            Self::Reset(messages) => Some(messages),
            Self::Batch(patches) => patches.iter().rev().find_map(TimelinePatch::last_reset),
            _ => None,
        }
    }

    fn adds_at_front(&self) -> bool {
        match self {
            Self::PushFront(_) => true,
            Self::Insert { index, .. } => *index == 0,
            Self::Batch(patches) => patches.iter().any(TimelinePatch::adds_at_front),
            _ => false,
        }
    }

    fn adds_at_back(&self) -> bool {
        match self {
            Self::Append(_) | Self::PushBack(_) => true,
            Self::Batch(patches) => patches.iter().any(TimelinePatch::adds_at_back),
            _ => false,
        }
    }
}

#[derive(Debug)]
pub enum TimelineCommand {
    PaginateBackwards,
    PaginateForwards,
    MarkRead,
    JumpTo(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineFocus {
    Live,
    Event(String),
}

impl TimelineFocus {
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live)
    }

    pub fn target(&self) -> Option<&str> {
        match self {
            Self::Live => None,
            Self::Event(event_id) => Some(event_id),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PaginationDirection {
    Backwards,
    Forwards,
}

#[derive(Debug, Clone, Copy)]
pub enum PaginationOutcome {
    Completed { hit_end: bool },
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineStatus {
    Loading,
    LoadingUnread,
    LoadingFocus,
    Ready,
    Failed { retryable: bool },
    Disconnected,
}

#[derive(Debug, Clone, Default)]
pub struct PaginationState {
    pub backwards_loading: bool,
    pub forwards_loading: bool,
}

#[derive(Debug, Clone)]
pub enum TimelineUpdate {
    Patch(Box<TimelinePatch>),
    ResolvingUnread,
    Pagination {
        direction: PaginationDirection,
        outcome: PaginationOutcome,
    },
    JumpOutcome {
        event_id: String,
        row: Option<usize>,
    },
}

impl TimelineUpdate {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Patch(patch) => patch.label(),
            Self::ResolvingUnread => "ResolvingUnread",
            Self::Pagination { .. } => "Pagination",
            Self::JumpOutcome { .. } => "JumpOutcome",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScrollMode {
    #[default]
    FollowLive,
    PreserveAnchor,
}

#[derive(Debug, Clone)]
pub struct VerificationEmoji {
    pub symbol: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub enum VerificationEvent {
    Requested { sender: String, is_self: bool },
    Emojis(Vec<VerificationEmoji>),
    Confirming,
    Done,
    Cancelled(VerificationCancellation),
}

#[derive(Debug, Clone)]
pub enum VerificationCancellation {
    TimedOut,
    AcceptFailed,
    Declined,
    Mismatch,
    AcceptedElsewhere,
    Remote,
    Failed,
}
