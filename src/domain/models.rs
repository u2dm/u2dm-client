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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginMethod {
    Password,
    OAuth,
    Both,
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
    pub homeserver_url: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LoginCredentials {
    pub homeserver: String,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionMetadata {
    pub user_id: String,
    pub device_id: String,
    pub homeserver: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

impl Session {
    pub fn metadata(&self) -> SessionMetadata {
        SessionMetadata {
            user_id: self.user_id.clone(),
            device_id: self.device_id.clone(),
            homeserver: self.homeserver.clone(),
            client_id: self.client_id.clone(),
        }
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
pub enum LastMessageKind {
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

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Room {
    pub id: RoomId,
    pub display_name: String,
    pub is_direct: bool,
    pub unread_count: u64,
    pub mention_count: u64,
    pub last_activity_ts: u64,
    pub last_message_sender: Option<String>,
    pub last_message_kind: LastMessageKind,
    pub last_message_body: String,
    pub last_message_is_own: bool,
}

#[derive(Debug, Clone)]
pub struct Space {
    pub id: String,
    pub name: String,
    pub avatar_mxc: Option<String>,
    pub child_room_ids: Vec<String>,
    pub unread: u64,
    pub mentions: u64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

#[derive(Debug)]
pub enum SyncEvent {
    Connected,
    Rooms(Vec<Room>),
    Spaces(Vec<Space>),
    ConnectionError(String),
    SessionExpired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventId(pub String);

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct ImageMeta {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub mimetype: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct FileMeta {
    pub filename: String,
    pub mimetype: Option<String>,
    pub size: Option<u64>,
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
    UnableToDecrypt,
    Unsupported {
        kind: String,
        fallback: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyInfo {
    pub sender: String,
    pub preview: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TimelineMessage {
    pub unique_id: String,
    pub event_id: EventId,
    pub sender: String,
    pub sender_display_name: Option<String>,
    pub sender_avatar_url: Option<String>,
    pub body: MessageBody,
    pub timestamp: u64,
    pub is_own: bool,
    pub reply: Option<ReplyInfo>,
}

impl TimelineMessage {
    pub fn visually_eq(&self, other: &Self) -> bool {
        self.unique_id == other.unique_id
            && self.sender == other.sender
            && self.sender_display_name == other.sender_display_name
            && self.sender_avatar_url == other.sender_avatar_url
            && self.body == other.body
            && self.timestamp == other.timestamp
            && self.is_own == other.is_own
            && self.reply == other.reply
    }
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
    UpdateMedia {
        event_id: EventId,
        message: TimelineMessage,
    },
}

impl TimelinePatch {
    pub fn label(&self) -> &'static str {
        self.into()
    }
}

#[derive(Debug)]
pub enum TimelineCommand {
    PaginateBackwards,
    PaginateForwards,
}

#[derive(Debug, Clone, Copy)]
pub enum PaginationDirection {
    Backwards,
    Forwards,
}

#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools, dead_code)]
pub struct PaginationState {
    pub backwards_ended: bool,
    pub forwards_ended: bool,
    pub backwards_loading: bool,
    pub forwards_loading: bool,
}

#[derive(Debug, Clone)]
pub enum TimelineUpdate {
    Patch(Box<TimelinePatch>),
    Pagination {
        direction: PaginationDirection,
        hit_end: bool,
    },
}

impl TimelineUpdate {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Patch(patch) => patch.label(),
            Self::Pagination { .. } => "Pagination",
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
    Cancelled(String),
}
