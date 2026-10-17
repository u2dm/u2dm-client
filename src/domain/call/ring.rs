use std::{fmt, ops};

use crate::domain::call::CallIntent;
use crate::domain::call::keys::ReceivedKey;
use crate::domain::member_name::NamedUser;
use crate::domain::room::RoomId;
use crate::domain::user_info::UserId;

pub const MAX_SENDER_TS_OFFSET_MS: u64 = 20_000;
pub const MAX_RING_LIFETIME_MS: u64 = 120_000;
pub const OUTGOING_RING_LIFETIME_MS: u64 = 90_000;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RingId(String);

impl RingId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl ops::Deref for RingId {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for RingId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingKind {
    Ring,
    Notify,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ring {
    pub id: RingId,
    pub room_id: RoomId,
    pub caller: NamedUser,
    pub intent: Option<CallIntent>,
    pub kind: RingKind,
    pub expires_ms: u64,
}

impl Ring {
    pub fn is_video(&self) -> bool {
        self.intent == Some(CallIntent::Video)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingRing {
    pub membership_event_id: String,
    pub kind: RingKind,
    pub intent: CallIntent,
    pub sent_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallSignal {
    Rang(Box<Ring>),
    Declined {
        room_id: RoomId,
        ring: RingId,
        by: UserId,
    },
    Key {
        room_id: RoomId,
        key: Box<ReceivedKey>,
    },
}

pub fn expires_ms(sender_ts: u64, origin_ts: u64, lifetime_ms: u64) -> u64 {
    let sent = if sender_ts.abs_diff(origin_ts) > MAX_SENDER_TS_OFFSET_MS {
        origin_ts
    } else {
        sender_ts
    };
    sent.saturating_add(lifetime_ms.min(MAX_RING_LIFETIME_MS))
}
