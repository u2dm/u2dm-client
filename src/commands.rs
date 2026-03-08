use crate::domain::models::{
    ConnectionStatus, LoginCredentials, Room, RoomId, ServerInfo, TimelinePatch, VerificationEvent,
};

pub enum UiCommand {
    RestoreSession,
    CheckServer(String),
    LoginPassword(LoginCredentials),
    LoginOAuth,
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
    LoginSuccess {
        user_id: String,
    },
    LoginError(String),
    ToastError(String),
    Status(String),
    Rooms(Vec<Room>),
    Timeline {
        room_id: RoomId,
        patch: Box<TimelinePatch>,
    },
    ConnectionStatus(ConnectionStatus),
    Verification(VerificationEvent),
    FileSaved {
        path: String,
    },
    LoggedOut,
}
