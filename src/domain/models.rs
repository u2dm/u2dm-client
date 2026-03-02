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
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::OAuth => "oauth",
            Self::Both => "both",
            Self::None => "",
        }
    }

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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub user_id: String,
    pub device_id: String,
    pub homeserver: String,
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// oauth client id present only for oauth sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomId(pub String);

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Room {
    pub id: RoomId,
    pub display_name: String,
    pub is_direct: bool,
    pub unread_count: u64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

impl ConnectionStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Error(_) => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SyncSnapshot {
    pub rooms: Vec<Room>,
    pub connection_status: ConnectionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventId(pub String);

#[derive(Debug, Clone)]
pub enum MessageBody {
    Text(String),
    Notice(String),
    Emote(String),
    Image(String),
    File(String),
    Unknown(String),
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TimelineMessage {
    pub event_id: EventId,
    pub sender: String,
    pub sender_display_name: Option<String>,
    pub body: MessageBody,
    pub timestamp: u64,
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
