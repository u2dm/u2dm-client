use strum::Display as StrumDisplay;

use crate::domain::models::{
    LoginCredentials, PaginationDirection, PaginationOutcome, RoomId, TimelineFocus,
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
    BackToHomeserver,
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
    #[strum(to_string = "SpaceOrderWriteFailed({op})")]
    SpaceOrderWriteFailed {
        op: u64,
        spaces: Vec<String>,
        error: String,
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
    #[strum(to_string = "JumpToEvent({event_id})")]
    JumpToEvent {
        event_id: String,
    },
    #[strum(to_string = "RefocusTimeline({room_id})")]
    RefocusTimeline {
        room_id: RoomId,
        generation: i32,
        focus: TimelineFocus,
    },
    RetryTimeline,
    SessionExpired,
    AcceptVerification,
    RejectVerification,
    ConfirmVerification,
    DismissVerification,
    #[strum(to_string = "OpenMedia({event_id})")]
    OpenMedia {
        event_id: String,
    },
    #[strum(to_string = "SaveFile({filename})")]
    SaveFile {
        event_id: String,
        filename: String,
    },
    DismissToast,
    Logout,
    Quit,
}

#[derive(Clone)]
pub struct ViewportChanged {
    pub room_id: RoomId,
    pub generation: i32,
    pub at_bottom: bool,
}

impl ViewportChanged {
    pub fn initial() -> Self {
        Self {
            room_id: RoomId::new(String::new()),
            generation: 0,
            at_bottom: true,
        }
    }
}
