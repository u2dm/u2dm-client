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

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Session {
    pub user_id: String,
    pub device_id: String,
    pub homeserver: String,
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
pub struct SyncSnapshot {
    pub rooms: Vec<Room>,
}
