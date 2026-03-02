use crate::domain::models::{LoginCredentials, RoomId};

pub enum UiCommand {
    CheckServer(String),
    LoginPassword(LoginCredentials),
    LoginOAuth(String),
    FetchRooms,
    SelectRoom(RoomId),
    SendMessage { room_id: RoomId, body: String },
}
