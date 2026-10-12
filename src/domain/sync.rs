use std::sync::Arc;

use crate::domain::invite::InviteList;
use crate::domain::room::{RoomList, Space};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncSupport {
    SlidingSync,
    ClassicOnly,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ConnectionStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error(String),
    SlidingSyncUnsupported,
}

#[derive(Debug)]
pub enum SyncEvent {
    Connected,
    Rooms(RoomList),
    Invites(InviteList),
    Spaces(Arc<[Space]>),
    ConnectionError(String),
    SlidingSyncUnsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReauthReason {
    SoftLogout,
    Refused,
}

#[derive(Debug, Clone, Copy)]
pub enum SessionLoss {
    Suspended(ReauthReason),
    Expired,
}

#[derive(Debug)]
pub enum SyncOutcome {
    Cancelled,
    Recoverable(String),
    SessionLost(SessionLoss),
    Fatal(String),
}
