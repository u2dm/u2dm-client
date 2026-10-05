use tokio::sync::watch;

use crate::domain::room::RoomId;
use crate::domain::room_log::RoomLog;

pub trait RoomLogPort: Send + Sync {
    fn read(&self, room_id: &RoomId) -> RoomLog;
    fn changes(&self) -> watch::Receiver<u64>;
    fn forget(&self);
}
