use std::sync::Arc;

use crate::domain::room::{RoomList, Space};

pub enum DirectoryUpdate {
    Rooms(RoomList),
    Spaces(Arc<[Space]>),
}
