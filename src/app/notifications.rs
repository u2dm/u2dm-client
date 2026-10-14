use std::mem;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use super::event::AppEvent;
use super::input::EventSender;
use super::room_directory::RoomDirectory;
use super::task_group::TaskGroup;
use crate::commands::ui::WindowFocus;
use crate::commands::view::{NotificationsView, ShownNotice};
use crate::domain::push::PushNotice;
use crate::domain::room::{Room, RoomId};
use crate::ports::matrix::PushPort;
use crate::ports::output::AppOutputPort;

const NOTICE_CHANNEL_CAP: usize = 16;
const HOLD_LIMIT: Duration = Duration::from_secs(10);

struct Held {
    notice: PushNotice,
    since: Instant,
}

pub(super) struct DesktopNotifications {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    focus: WindowFocus,
    serial: u64,
    shown: Vec<ShownNotice>,
    held: Vec<Held>,
}

impl DesktopNotifications {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            focus: WindowFocus::default(),
            serial: 0,
            shown: Vec::new(),
            held: Vec::new(),
        }
    }

    pub(super) fn spawn_listener(
        &self,
        group: &mut TaskGroup,
        port: Arc<dyn PushPort>,
        session: u64,
    ) {
        let events = self.events.clone();
        let token = group.token();
        group.spawn(async move {
            let (notices_tx, notices_rx) = mpsc::channel(NOTICE_CHANNEL_CAP);
            let follow = async {
                tokio::join!(
                    follow_notices(port.as_ref(), notices_tx),
                    forward_notices(&events, session, notices_rx),
                )
            };
            token.run_until_cancelled(follow).await;
        });
    }

    pub(super) fn arrived(&mut self, notice: PushNotice, directory: &RoomDirectory) {
        let room_id = notice.room_id.clone();
        if self.focus == WindowFocus::Focused {
            tracing::debug!(%room_id, "the window has focus; no notification");
            return;
        }
        if self.supersedes(&notice) {
            tracing::debug!(%room_id, "the room already notified a newer message");
            return;
        }
        self.held.retain(|held| held.notice.room_id != room_id);
        self.held.push(Held {
            notice,
            since: Instant::now(),
        });
        let released = self.release_confirmed(directory);
        if self.held.iter().any(|held| held.notice.room_id == room_id) {
            tracing::debug!(%room_id, "held a notification until its room counts as unread");
        }
        if released {
            self.publish();
        }
    }

    pub(super) fn directory_changed(&mut self, directory: &RoomDirectory) {
        let before = self.shown.len();
        self.shown.retain(|shown| {
            let unread = still_unread(&shown.notice, directory);
            if !unread {
                tracing::debug!(room_id = %shown.notice.room_id, "the room was read; withdrawing its notification");
            }
            unread
        });
        let withdrew = self.shown.len() != before;
        if self.release_confirmed(directory) || withdrew {
            self.publish();
        }
    }

    pub(super) fn focus_changed(&mut self, focus: WindowFocus) {
        if self.focus == focus {
            return;
        }
        self.focus = focus;
        tracing::debug!(?focus, "the window's focus changed");
        if focus == WindowFocus::Focused && !self.held.is_empty() {
            tracing::debug!(
                held = self.held.len(),
                "the window has focus; dropping held notifications"
            );
            self.held.clear();
        }
    }

    pub(super) fn on_screen(&mut self, room_id: Option<&RoomId>) {
        let Some(room_id) = room_id else {
            return;
        };
        self.held.retain(|held| held.notice.room_id != *room_id);
        let before = self.shown.len();
        self.shown.retain(|shown| shown.notice.room_id != *room_id);
        if self.shown.len() != before {
            tracing::debug!(%room_id, "the room is on screen; withdrawing its notification");
            self.publish();
        }
    }

    pub(super) fn reset(&mut self) {
        self.shown.clear();
        self.held.clear();
    }

    fn supersedes(&self, notice: &PushNotice) -> bool {
        self.shown
            .iter()
            .map(|shown| &shown.notice)
            .chain(self.held.iter().map(|held| &held.notice))
            .any(|other| other.room_id == notice.room_id && notice.is_older_than(other))
    }

    fn release_confirmed(&mut self, directory: &RoomDirectory) -> bool {
        let now = Instant::now();
        let mut released = false;
        for held in mem::take(&mut self.held) {
            if now.duration_since(held.since) > HOLD_LIMIT {
                tracing::debug!(room_id = %held.notice.room_id, "the room never counted as unread; dropping its notification");
            } else if still_unread(&held.notice, directory) {
                self.show(held.notice);
                released = true;
            } else {
                self.held.push(held);
            }
        }
        released
    }

    fn show(&mut self, notice: PushNotice) {
        self.serial = self.serial.wrapping_add(1);
        self.shown
            .retain(|shown| shown.notice.room_id != notice.room_id);
        tracing::debug!(room_id = %notice.room_id, serial = self.serial, "a notification shows");
        self.shown.push(ShownNotice {
            serial: self.serial,
            notice,
        });
    }

    fn publish(&self) {
        let shown: Arc<[ShownNotice]> = self.shown.clone().into();
        self.output.publish(Box::new(move |view| {
            view.notifications = NotificationsView { shown };
        }));
    }
}

fn still_unread(notice: &PushNotice, directory: &RoomDirectory) -> bool {
    if notice.is_invite() {
        return directory.invite(&notice.room_id).is_some();
    }
    directory.room(&notice.room_id).is_some_and(Room::alert)
}

async fn follow_notices(port: &dyn PushPort, notices_tx: mpsc::Sender<PushNotice>) {
    if let Err(e) = port.follow_notifications(notices_tx).await {
        tracing::warn!("failed to follow notifications: {e}");
    }
}

async fn forward_notices(
    events: &EventSender,
    session: u64,
    mut notices_rx: mpsc::Receiver<PushNotice>,
) {
    while let Some(notice) = notices_rx.recv().await {
        let event = AppEvent::Notified {
            session,
            notice: Box::new(notice),
        };
        if events.send(event).is_err() {
            return;
        }
    }
}
