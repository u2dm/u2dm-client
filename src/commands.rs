use std::path::PathBuf;
use std::sync::Arc;

use strum::Display as StrumDisplay;

use crate::domain::models::{
    ConnectionStatus, LoginCredentials, LoginMethod, PaginationDirection, PaginationOutcome, Room,
    RoomId, Space, TimelinePatch, TimelineStatus, VerificationEvent,
};

pub enum DirectoryUpdate {
    Rooms(Arc<[Room]>),
    Spaces(Arc<[Space]>),
}

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

pub enum Effect {
    Snapshot(Arc<AppViewState>),
    LoginError(String),
    Toast(String),
    Status(String),
    SelectedRoom {
        id: RoomId,
        name: String,
        member_count: u64,
        generation: i32,
    },
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

#[derive(Clone, Default)]
pub struct AppViewState {
    pub lifecycle: LifecycleView,
    pub connection: ConnectionStatus,
    pub directory: DirectoryView,
    pub pagination: PaginationView,
}

impl AppViewState {
    pub fn logged_out() -> Self {
        Self::default()
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct PaginationView {
    pub generation: i32,
    pub backwards_loading: bool,
    pub forwards_loading: bool,
    pub new_messages: u32,
}

impl PaginationView {
    pub fn retarget(&mut self, generation: i32) {
        if self.generation != generation {
            *self = Self {
                generation,
                ..Self::default()
            };
        }
    }
}

#[derive(Clone, Default)]
pub struct LifecycleView {
    pub step: LoginStep,
    pub method: LoginMethod,
    pub resolved_homeserver: String,
    pub user_id: String,
    pub avatar_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum LoginStep {
    #[default]
    Homeserver,
    Credentials,
    LoggedIn,
}

#[derive(Clone)]
pub struct DirectoryView {
    pub rooms: Arc<[Room]>,
    pub spaces: Arc<[Space]>,
    pub subspaces: Arc<[Space]>,
    pub space_id: String,
    pub subspace_id: String,
}

impl Default for DirectoryView {
    fn default() -> Self {
        Self {
            rooms: Arc::from(Vec::new()),
            spaces: Arc::from(Vec::new()),
            subspaces: Arc::from(Vec::new()),
            space_id: String::new(),
            subspace_id: String::new(),
        }
    }
}
