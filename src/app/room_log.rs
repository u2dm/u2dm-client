use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;

use super::event::AppEvent;
use super::input::EventSender;
use super::task_group::TaskGroup;
use crate::commands::view::RoomLogView;
use crate::domain::room::RoomId;
use crate::ports::output::AppOutputPort;
use crate::ports::room_log::RoomLogPort;

const REREAD_AT_MOST_EVERY: Duration = Duration::from_millis(250);

pub(super) struct RoomLogViewer {
    port: Arc<dyn RoomLogPort>,
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    follower: TaskGroup,
    landed: i32,
    shown: Option<RoomLogView>,
}

impl RoomLogViewer {
    pub(super) fn new(
        port: Arc<dyn RoomLogPort>,
        output: Arc<dyn AppOutputPort>,
        events: EventSender,
    ) -> Self {
        Self {
            port,
            output,
            events,
            follower: TaskGroup::new("room-log"),
            landed: 0,
            shown: None,
        }
    }

    pub(super) fn open(&mut self, room_id: RoomId, name: String) {
        let log = self.port.read(&room_id);
        let followed = self.shown.is_some();
        self.landed = self.landed.wrapping_add(1);
        self.shown = Some(RoomLogView {
            room_id,
            name,
            log,
            lines_landed: self.landed,
        });
        self.publish();
        if !followed {
            self.follow();
        }
    }

    pub(super) fn reread(&mut self) {
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        let log = self.port.read(&shown.room_id);
        if log.holds_the_same_lines_as(&shown.log) {
            return;
        }
        self.landed = self.landed.wrapping_add(1);
        shown.log = log;
        shown.lines_landed = self.landed;
        self.publish();
    }

    pub(super) fn close(&mut self) {
        self.follower.cancel_and_detach();
        if self.shown.take().is_some() {
            self.publish();
        }
    }

    pub(super) fn forget(&mut self) {
        self.follower.cancel_and_detach();
        self.shown = None;
        self.port.forget();
    }

    pub(super) async fn restart(&mut self) {
        self.follower.restart().await;
    }

    pub(super) async fn shutdown(&mut self) {
        self.follower.shutdown().await;
    }

    fn follow(&mut self) {
        let mut changes = self.port.changes();
        let events = self.events.clone();
        let token = self.follower.token();
        self.follower.spawn(async move {
            let relay = async {
                while changes.changed().await.is_ok() {
                    if events.send(AppEvent::RoomLogGrew).is_err() {
                        return;
                    }
                    sleep(REREAD_AT_MOST_EVERY).await;
                }
            };
            token.run_until_cancelled(relay).await;
        });
    }

    fn publish(&self) {
        let shown = self.shown.clone();
        self.output
            .publish(Box::new(move |view| view.room_log = shown));
    }
}
