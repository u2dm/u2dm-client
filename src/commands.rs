use strum::Display as StrumDisplay;

use crate::domain::models::{
    ConnectionStatus, LoginCredentials, Room, RoomId, ServerInfo, TimelinePatch, VerificationEvent,
};

#[derive(StrumDisplay)]
pub enum UiCommand {
    RestoreSession,
    #[strum(to_string = "CheckServer({0})")]
    CheckServer(String),
    #[strum(to_string = "LoginPassword(...)")]
    LoginPassword(LoginCredentials),
    LoginOAuth,
    FetchRooms,
    #[strum(to_string = "SelectRoom({0})")]
    SelectRoom(RoomId),
    #[strum(to_string = "SendMessage({room_id})")]
    SendMessage {
        room_id: RoomId,
        body: String,
    },
    SessionExpired,
    AcceptVerification,
    RejectVerification,
    ConfirmVerification,
    #[strum(to_string = "OpenMedia({event_id})")]
    OpenMedia {
        event_id: String,
    },
    #[strum(to_string = "SaveFile({filename})")]
    SaveFile {
        event_id: String,
        filename: String,
    },
    Logout,
    Quit,
}

pub enum UiEvent {
    ServerInfo(ServerInfo),
    ShowLogin,
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
