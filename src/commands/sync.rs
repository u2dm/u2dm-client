use std::sync::Arc;

use crate::domain::models::{RoomList, Space};

pub enum DirectoryUpdate {
    Rooms(RoomList),
    Spaces(Arc<[Space]>),
}
