use std::path::PathBuf;

use strum::Display as StrumDisplay;

use crate::domain::models::{
    ConnectionStatus, LoginCredentials, PaginationDirection, PaginationOutcome, PaginationState,
    Room, RoomId, ServerInfo, Space, TimelinePatch, TimelineStatus, VerificationEvent,
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
    #[strum(to_string = "RoomsUpdated")]
    RoomsUpdated(Vec<Room>),
    #[strum(to_string = "SpacesUpdated")]
    SpacesUpdated(Vec<Space>),
    #[strum(to_string = "SelectSpace")]
    SelectSpace(Option<RoomId>),
    #[strum(to_string = "SelectSubspace")]
    SelectSubspace(Option<RoomId>),
    #[strum(to_string = "MoveSpace({from},{to})")]
    MoveSpace {
        from: usize,
        to: usize,
    },
    #[strum(to_string = "SelectRoom({0})")]
    SelectRoom(RoomId),
    #[strum(to_string = "SendMessage({room_id})")]
    SendMessage {
        room_id: RoomId,
        body: String,
        reply_to: Option<String>,
    },
    #[strum(to_string = "PaginateBackwards({room_id})")]
    PaginateBackwards {
        room_id: RoomId,
    },
    #[strum(to_string = "PaginateForwards({room_id})")]
    PaginateForwards {
        room_id: RoomId,
    },
    #[strum(to_string = "TimelinePaginationCompleted({room_id})")]
    TimelinePaginationCompleted {
        room_id: RoomId,
        direction: PaginationDirection,
        outcome: PaginationOutcome,
    },
    #[strum(to_string = "JumpToLatest({room_id})")]
    JumpToLatest {
        room_id: RoomId,
    },
    #[strum(to_string = "ScrollPositionChanged")]
    ScrollPositionChanged {
        at_top: bool,
        at_bottom: bool,
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
    UserAvatar(Option<PathBuf>),
    LoginError(String),
    ToastError(String),
    Status(String),
    Rooms(Vec<Room>),
    Spaces(Vec<Space>),
    Subspaces(Vec<Space>),
    Timeline {
        room_id: RoomId,
        patch: Box<TimelinePatch>,
    },
    TimelineStatus {
        room_id: RoomId,
        status: TimelineStatus,
    },
    PaginationState {
        room_id: RoomId,
        state: PaginationState,
    },
    NewMessagesBadge {
        room_id: RoomId,
        count: u32,
    },
    ScrollToBottom {
        room_id: RoomId,
    },
    ConnectionStatus(ConnectionStatus),
    Verification(VerificationEvent),
    FileSaved {
        path: String,
    },
    LoggedOut,
}
