use std::sync::Arc;

use crate::domain::member_name::NamedUser;
use crate::domain::room::RoomId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteKind {
    Room,
    Space,
    Direct,
}

impl InviteKind {
    pub fn of(is_space: bool, is_direct: bool) -> Self {
        if is_space {
            Self::Space
        } else if is_direct {
            Self::Direct
        } else {
            Self::Room
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub id: RoomId,
    pub name: String,
    pub avatar_mxc: Option<String>,
    pub topic: Option<String>,
    pub kind: InviteKind,
    pub inviter: Option<NamedUser>,
    pub received_ts: u64,
}

impl Invite {
    pub fn is_direct(&self) -> bool {
        self.kind == InviteKind::Direct
    }

    pub fn is_space(&self) -> bool {
        self.kind == InviteKind::Space
    }
}

pub type InviteList = Arc<[Arc<Invite>]>;
