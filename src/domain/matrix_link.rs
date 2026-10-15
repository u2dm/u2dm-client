use std::fmt;

use crate::domain::room::RoomId;
use crate::domain::timeline::TimelineFocus;
use crate::domain::user_info::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatrixLink {
    User(UserId),
    Room(RoomLink),
}

impl MatrixLink {
    pub fn into_user(self) -> Option<UserId> {
        match self {
            Self::User(user_id) => Some(user_id),
            Self::Room(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomLink {
    pub room: RoomAddress,
    pub event: Option<String>,
    pub via: Vec<String>,
}

impl RoomLink {
    pub fn to_room(room: RoomAddress) -> Self {
        Self {
            room,
            event: None,
            via: Vec::new(),
        }
    }

    pub fn focus(&self) -> TimelineFocus {
        self.event
            .clone()
            .map_or(TimelineFocus::ReadPosition, TimelineFocus::Event)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RoomAddress {
    Id(RoomId),
    Alias(String),
}

impl RoomAddress {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Id(room_id) => room_id,
            Self::Alias(alias) => alias,
        }
    }
}

impl fmt::Display for RoomAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressEntry {
    Room(RoomLink),
    Unrecognized,
}
