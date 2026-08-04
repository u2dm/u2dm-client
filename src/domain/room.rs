use std::sync::Arc;
use std::{fmt, ops};

use crate::domain::message::{MessagePreviewKind, ServiceEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomId(String);

impl RoomId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl ops::Deref for RoomId {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for RoomId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RoomId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NotifyMode {
    #[default]
    AllMessages,
    MentionsOnly,
    Muted,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Room {
    pub id: RoomId,
    pub display_name: String,
    pub avatar_mxc: Option<String>,
    pub is_direct: bool,
    pub member_count: u64,
    pub has_unread: bool,
    pub has_mentions: bool,
    pub has_activity: bool,
    pub notify: NotifyMode,
    pub last_activity_ts: u64,
    pub last_message_sender: Option<String>,
    pub last_message_kind: MessagePreviewKind,
    pub last_message_body: String,
    pub last_message_service: Option<ServiceEvent>,
    pub last_message_is_own: bool,
    pub last_message_edited: bool,
}

impl Room {
    pub fn muted(&self) -> bool {
        matches!(self.notify, NotifyMode::Muted)
    }

    pub fn alert(&self) -> bool {
        !self.muted() && (self.has_unread || self.has_mentions)
    }

    pub fn mention(&self) -> bool {
        self.has_mentions && !self.muted()
    }

    pub fn hint(&self) -> bool {
        self.has_activity && !self.alert()
    }
}

pub type RoomList = Arc<[Arc<Room>]>;

#[derive(Debug, Clone, PartialEq)]
pub struct Space {
    pub id: String,
    pub name: String,
    pub avatar_mxc: Option<String>,
    pub child_room_ids: Vec<String>,
    pub child_space_ids: Vec<String>,
    pub order: Option<String>,
    pub alert: bool,
    pub mention: bool,
    pub hint: bool,
}
