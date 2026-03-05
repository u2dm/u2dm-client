use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiErrorKind {
    Authentication,
    Network,
    Storage,
    Other,
}

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

#[derive(Clone)]
pub struct Session {
    pub user_id: String,
    pub device_id: String,
    pub homeserver: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// oauth client id present only for oauth sessions.
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

/// non-secret session metadata safe for disk storage.
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
pub struct RoomId(pub String);

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Room {
    pub id: RoomId,
    pub display_name: String,
    pub is_direct: bool,
    pub unread_count: u64,
    pub last_activity_ts: u64,
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
