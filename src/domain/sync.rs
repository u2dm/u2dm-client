use std::sync::Arc;

use crate::domain::room::{RoomList, Space};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ConnectionStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

#[derive(Debug)]
pub enum SyncEvent {
    Connected,
    Rooms(RoomList),
    Spaces(Arc<[Space]>),
    ConnectionError(String),
}

#[derive(Debug)]
pub enum SyncOutcome {
    Cancelled,
    Recoverable(String),
    SessionExpired,
    Fatal(String),
}
