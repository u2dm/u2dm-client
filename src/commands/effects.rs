use std::sync::Arc;

use super::messages::UserMessage;
use super::view::AppViewState;
use crate::domain::models::{RoomId, TimelinePatch, TimelineStatus, VerificationEvent};

pub enum Effect {
    Snapshot(Arc<AppViewState>),
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
    TimelineFocus {
        room_id: RoomId,
        generation: i32,
        event_id: String,
        row: usize,
    },
    Verification(VerificationUpdate),
    LoggedOut,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum VerificationActivity {
    #[default]
    None,
    Accepting,
    Declining,
    Confirming,
}

pub enum VerificationUpdate {
    Flow(VerificationEvent),
    Busy(VerificationActivity),
    Failed(UserMessage),
    Dismissed,
}
