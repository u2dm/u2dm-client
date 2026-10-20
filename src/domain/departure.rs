use std::sync::Arc;

use crate::domain::member_name::NamedUser;
use crate::domain::room::RoomId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalKind {
    Kicked,
    Banned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub id: RoomId,
    pub name: String,
    pub avatar_mxc: Option<String>,
    pub kind: RemovalKind,
    pub is_space: bool,
    pub is_direct: bool,
    pub by: NamedUser,
    pub reason: Option<String>,
    pub removed_ts: u64,
}

pub type RemovalList = Arc<[Arc<Removal>]>;

pub fn membership_reason(typed: String) -> Option<String> {
    let reason = typed.trim();
    if reason.is_empty() {
        return None;
    }
    if reason.len() == typed.len() {
        return Some(typed);
    }
    Some(reason.to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminStanding {
    OnlyAdmin,
    NotOnlyAdmin,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpaceDeparture {
    pub rooms: Vec<RoomId>,
    pub spaces: Vec<RoomId>,
}
