use std::path::PathBuf;
use std::sync::Arc;

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
    CancelOAuth,
    FetchRooms,
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
        generation: i32,
    },
    #[strum(to_string = "PaginateForwards({room_id})")]
    PaginateForwards {
        room_id: RoomId,
        generation: i32,
    },
    #[strum(to_string = "TimelinePaginationCompleted({room_id})")]
    TimelinePaginationCompleted {
        room_id: RoomId,
        generation: i32,
        direction: PaginationDirection,
        outcome: PaginationOutcome,
    },
    #[strum(to_string = "JumpToLatest({room_id})")]
    JumpToLatest {
        room_id: RoomId,
        generation: i32,
    },
    RetryTimeline,
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
    Rooms(Arc<[Room]>),
    Spaces(Arc<[Space]>),
    Subspaces(Arc<[Space]>),
    SelectedRoom {
        id: RoomId,
        name: String,
        member_count: u64,
        generation: i32,
    },
    SelectedSpace(String),
    SelectedSubspace(String),
    Timeline {
        room_id: RoomId,
        generation: i32,
        patch: Box<TimelinePatch>,
    },
    TimelineStatus {
        room_id: RoomId,
        generation: i32,
        status: TimelineStatus,
    },
    PaginationState {
        room_id: RoomId,
        generation: i32,
        state: PaginationState,
    },
    NewMessagesBadge {
        room_id: RoomId,
        generation: i32,
        count: u32,
    },
    ScrollToBottom {
        room_id: RoomId,
        generation: i32,
    },
    ConnectionStatus(ConnectionStatus),
    Verification(VerificationEvent),
    FileSaved {
        path: String,
    },
    LoggedOut,
}

#[derive(Clone)]
pub struct ViewportChanged {
    pub room_id: RoomId,
    pub generation: i32,
    pub at_top: bool,
    pub at_bottom: bool,
}

impl ViewportChanged {
    pub fn initial() -> Self {
        Self {
            room_id: RoomId::new(String::new()),
            generation: 0,
            at_top: false,
            at_bottom: true,
        }
    }
}
