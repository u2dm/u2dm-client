use crate::domain::models::{
    ConnectionStatus, LoginCredentials, Room, RoomId, ServerInfo, TimelinePatch, UiErrorKind,
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
    OpenMedia { event_id: String },
    SaveFile { event_id: String, filename: String },
    Logout,
    Quit,
}

pub enum UiEvent {
    ServerInfo(ServerInfo),
    LoginSuccess { user_id: String },
    Error { message: String, kind: UiErrorKind },
    Status(String),
    Rooms(Vec<Room>),
    Timeline(TimelinePatch),
    ConnectionStatus(ConnectionStatus),
    Verification(VerificationEvent),
    FileSaved { path: String },
    LoggedOut,
}
