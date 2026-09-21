use std::collections::HashMap;
use std::future::Future;

use tokio::sync::oneshot;

use super::task_group::TaskGroup;
use crate::domain::room::RoomId;

pub(super) struct SendLanes {
    tasks: TaskGroup,
    tails: HashMap<RoomId, oneshot::Receiver<()>>,
}

impl SendLanes {
    pub(super) fn new() -> Self {
        Self {
            tasks: TaskGroup::new("sends"),
            tails: HashMap::new(),
        }
    }

    pub(super) fn spawn(
        &mut self,
        room_id: RoomId,
        send: impl Future<Output = ()> + Send + 'static,
    ) {
        let (finished, tail) = oneshot::channel::<()>();
        let ahead = self.tails.insert(room_id, tail);
        self.tasks.spawn(async move {
            if let Some(ahead) = ahead {
                drop(ahead.await);
            }
            send.await;
            drop(finished);
        });
    }

    pub(super) async fn restart(&mut self) {
        self.tails.clear();
        self.tasks.restart().await;
    }

    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
    }
}
