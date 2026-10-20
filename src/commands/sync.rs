use std::sync::Arc;

use crate::domain::departure::RemovalList;
use crate::domain::invite::InviteList;
use crate::domain::room::{RoomList, Space};

pub enum DirectoryUpdate {
    Rooms(RoomList),
    Invites(InviteList),
    Removals(RemovalList),
    Spaces(Arc<[Space]>),
}
