use strum::Display as StrumDisplay;

use crate::domain::room::RoomId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, StrumDisplay)]
pub enum AudioEnd {
    Finished,
    Failed,
}

#[derive(Clone)]
pub struct ViewportChanged {
    pub room_id: RoomId,
    pub generation: i32,
    pub at_bottom: bool,
    pub unread_below: u32,
}

impl ViewportChanged {
    pub fn initial() -> Self {
        Self {
            room_id: RoomId::new(String::new()),
            generation: 0,
            at_bottom: true,
            unread_below: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TimelineVisibility {
    Visible,
    #[default]
    Hidden,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WindowFocus {
    #[default]
    Focused,
    Unfocused,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UserActivity {
    #[default]
    Active,
    Idle,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowAttention {
    pub focus: WindowFocus,
    pub activity: UserActivity,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposerActivity {
    pub room_id: Option<RoomId>,
    pub composing: bool,
    pub edits: i32,
}
