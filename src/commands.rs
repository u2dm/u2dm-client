use crate::domain::models::{LoginCredentials, Room, RoomId, ServerInfo, TimelineMessage};

pub enum UiCommand {
    RestoreSession,
    CheckServer(String),
    LoginPassword(LoginCredentials),
    LoginOAuth(String),
    FetchRooms,
    SelectRoom(RoomId),
    SendMessage { room_id: RoomId, body: String },
}

pub enum UiEvent {
    ServerInfo(ServerInfo),
    LoginSuccess { user_id: String },
    Error(String),
    Status(String),
    Rooms(Vec<Room>),
    Timeline(Vec<TimelineMessage>),
}
