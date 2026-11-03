use std::sync::Arc;

use async_trait::async_trait;

use super::event::AppEvent;
use super::input::EventSender;
use super::task_group::TaskGroup;
use crate::ports::matrix::{InvitePeoplePort, RoomInfoPort, SpaceIndexPort};

pub(super) const AVATAR_BATCH: usize = 8;

#[async_trait]
pub(super) trait AvatarSource: Send + Sync {
    async fn fetch_batch(&self, mxcs: &[String]) -> usize;
}

#[async_trait]
impl AvatarSource for dyn RoomInfoPort {
    async fn fetch_batch(&self, mxcs: &[String]) -> usize {
        self.fetch_avatars(mxcs).await
    }
}

#[async_trait]
impl AvatarSource for dyn InvitePeoplePort {
    async fn fetch_batch(&self, mxcs: &[String]) -> usize {
        self.fetch_avatars(mxcs).await
    }
}

#[async_trait]
impl AvatarSource for dyn SpaceIndexPort {
    async fn fetch_batch(&self, mxcs: &[String]) -> usize {
        self.fetch_avatars(mxcs).await
    }
}

pub(super) fn spawn_avatar_batches<P: AvatarSource + ?Sized + 'static>(
    group: &mut TaskGroup,
    events: &EventSender,
    port: Arc<P>,
    mxcs: Vec<String>,
    landed: impl Fn(usize) -> AppEvent + Send + 'static,
) {
    if mxcs.is_empty() {
        return;
    }
    let events = events.clone();
    let cancel = group.token();
    group.spawn(async move {
        let fetch = async move {
            for batch in mxcs.chunks(AVATAR_BATCH) {
                let ready = port.fetch_batch(batch).await;
                if ready > 0 {
                    drop(events.send(landed(ready)));
                }
            }
        };
        cancel.run_until_cancelled(fetch).await;
    });
}
