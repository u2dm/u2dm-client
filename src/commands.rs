use crate::domain::models::{
    ConnectionStatus, LoginCredentials, Room, RoomId, ServerInfo, TimelineMessage,
    VerificationEvent,
};

pub enum UiCommand {
    RestoreSession,
    CheckServer(String),
    LoginPassword(LoginCredentials),
    LoginOAuth(String),
    FetchRooms,
    SelectRoom(RoomId),
    SendMessage { room_id: RoomId, body: String },
    SessionExpired,
    AcceptVerification,
    RejectVerification,
    ConfirmVerification,
    Logout,
    Quit,
}

pub enum UiEvent {
    ServerInfo(ServerInfo),
    LoginSuccess { user_id: String },
    Error(String),
    Status(String),
    Rooms(Vec<Room>),
    Timeline(Vec<TimelineMessage>),
    ConnectionStatus(ConnectionStatus),
    Verification(VerificationEvent),
    LoggedOut,
}
