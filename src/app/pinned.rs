use std::sync::Arc;

use tokio::sync::mpsc;

use super::event::AppEvent;
use super::input::EventSender;
use super::task_group::TaskGroup;
use crate::commands::view::PinnedView;
use crate::domain::message::PinnedMessage;
use crate::domain::room::RoomId;
use crate::ports::matrix::PinnedPort;
use crate::ports::output::AppOutputPort;

const PINNED_CHANNEL_CAP: usize = 8;

#[derive(Default)]
enum Shown {
    #[default]
    Newest,
    Event(String),
}

pub(super) struct PinnedMessages {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    tasks: TaskGroup,
    watch: u64,
    room_id: Option<RoomId>,
    messages: Arc<[PinnedMessage]>,
    shown: Shown,
}

impl PinnedMessages {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            tasks: TaskGroup::new("pinned"),
            watch: 0,
            room_id: None,
            messages: Arc::default(),
            shown: Shown::Newest,
        }
    }

    pub(super) fn follow(&mut self, port: Arc<dyn PinnedPort>, room_id: RoomId) {
        self.tasks.cancel_and_detach();
        self.watch = self.watch.wrapping_add(1);
        if self.room_id.as_ref() != Some(&room_id) {
            self.forget();
            self.room_id = Some(room_id.clone());
            self.publish();
        }
        tracing::debug!(%room_id, watch = self.watch, "following the pinned messages");

        let watch = self.watch;
        let events = self.events.clone();
        let token = self.tasks.token();
        self.tasks.spawn(async move {
            let (pinned_tx, pinned_rx) = mpsc::channel(PINNED_CHANNEL_CAP);
            let follow = async {
                tokio::join!(
                    subscribe(port.as_ref(), &room_id, pinned_tx),
                    forward(&events, watch, pinned_rx),
                )
            };
            token.run_until_cancelled(follow).await;
        });
    }

    pub(super) fn changed(&mut self, watch: u64, messages: Vec<PinnedMessage>) {
        if watch != self.watch {
            tracing::debug!(
                watch,
                "dropping pinned messages from a superseded subscription"
            );
            return;
        }
        self.messages = Arc::from(messages);
        if let Shown::Event(event_id) = &self.shown
            && self.position_of(event_id).is_none()
        {
            self.shown = Shown::Newest;
        }
        tracing::debug!(pinned = self.messages.len(), "the pinned messages changed");
        self.publish();
    }

    pub(super) fn opened(&mut self, event_id: &str) {
        let Some(opened) = self.position_of(event_id) else {
            return;
        };
        let older = opened.checked_sub(1).and_then(|row| self.messages.get(row));
        self.shown = older.map_or(Shown::Newest, |older| Shown::Event(older.event_id.clone()));
        tracing::debug!(opened, "showing the pinned message before the one opened");
        self.publish();
    }

    pub(super) fn clear(&mut self) {
        self.tasks.cancel_and_detach();
        self.watch = self.watch.wrapping_add(1);
        self.forget();
        self.publish();
    }

    pub(super) async fn restart(&mut self) {
        self.tasks.restart().await;
        self.watch = self.watch.wrapping_add(1);
        self.forget();
    }

    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
    }

    fn forget(&mut self) {
        self.room_id = None;
        self.messages = Arc::default();
        self.shown = Shown::Newest;
    }

    fn position_of(&self, event_id: &str) -> Option<usize> {
        self.messages
            .iter()
            .position(|message| message.event_id == event_id)
    }

    fn shown_index(&self) -> usize {
        let newest = self.messages.len().saturating_sub(1);
        match &self.shown {
            Shown::Newest => newest,
            Shown::Event(event_id) => self.position_of(event_id).unwrap_or(newest),
        }
    }

    fn publish(&self) {
        let view = PinnedView {
            room_id: self.room_id.clone(),
            messages: Arc::clone(&self.messages),
            shown: self.shown_index(),
        };
        self.output
            .publish(Box::new(move |state| state.pinned = view));
    }
}

async fn subscribe(
    port: &dyn PinnedPort,
    room_id: &RoomId,
    pinned_tx: mpsc::Sender<Vec<PinnedMessage>>,
) {
    if let Err(e) = port.subscribe_pinned(room_id, pinned_tx).await {
        tracing::warn!(%room_id, "failed to follow the pinned messages: {e}");
    }
}

async fn forward(
    events: &EventSender,
    watch: u64,
    mut pinned_rx: mpsc::Receiver<Vec<PinnedMessage>>,
) {
    while let Some(messages) = pinned_rx.recv().await {
        if events
            .send(AppEvent::PinnedChanged { watch, messages })
            .is_err()
        {
            return;
        }
    }
}
