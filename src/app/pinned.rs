use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use tokio::sync::mpsc;

use super::event::{AppEvent, MessageActionEvent};
use super::input::EventSender;
use super::room_info::ActionOutcome;
use super::send_lanes::SendLanes;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{PinnedView, Toast};
use crate::domain::message::{PinChange, PinnedMessage};
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum PinPhase {
    Requested(u64),
    AwaitingEcho,
}

struct PendingPin {
    room_id: RoomId,
    target: PinChange,
    phase: PinPhase,
}

impl PendingPin {
    fn pins(&self) -> bool {
        self.target == PinChange::Pin
    }
}

pub(super) struct PinnedMessages {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    tasks: TaskGroup,
    pins: SendLanes,
    watch: u64,
    requests: u64,
    room_id: Option<RoomId>,
    messages: Arc<[PinnedMessage]>,
    shown: Shown,
    changes: HashMap<String, PendingPin>,
}

impl PinnedMessages {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            tasks: TaskGroup::new("pinned"),
            pins: SendLanes::new("pins"),
            watch: 0,
            requests: 0,
            room_id: None,
            messages: Arc::default(),
            shown: Shown::Newest,
            changes: HashMap::new(),
        }
    }

    pub(super) fn follow(&mut self, port: Arc<dyn PinnedPort>, room_id: RoomId) {
        self.tasks.cancel_and_detach();
        self.watch = self.watch.wrapping_add(1);
        if self.room_id.as_ref() != Some(&room_id) {
            self.forget();
            self.keep_requested_changes();
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
        self.settle_echoed_changes();
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

    pub(super) fn change(
        &mut self,
        port: Arc<dyn PinnedPort>,
        room_id: RoomId,
        event_id: String,
        change: PinChange,
    ) {
        if self.shows_pinned(&room_id, &event_id) == (change == PinChange::Pin) {
            tracing::debug!(%room_id, event_id, ?change, "the pin already says so");
            return;
        }
        self.requests = self.requests.wrapping_add(1);
        let request = self.requests;
        self.changes.insert(
            event_id.clone(),
            PendingPin {
                room_id: room_id.clone(),
                target: change,
                phase: PinPhase::Requested(request),
            },
        );
        self.publish();
        let events = self.events.clone();
        self.pins.spawn(room_id.clone(), async move {
            let outcome = match port.change_pin(&room_id, &event_id, change).await {
                Ok(()) => ActionOutcome::Done,
                Err(e) => {
                    tracing::warn!(%room_id, event_id, ?change, "failed to change a pin: {e}");
                    ActionOutcome::Failed
                }
            };
            let settled = MessageActionEvent::PinSettled {
                request,
                event_id,
                outcome,
            };
            drop(events.send(AppEvent::MessageAction(settled)));
        });
    }

    pub(super) fn pin_settled(&mut self, request: u64, event_id: &str, outcome: ActionOutcome) {
        let Some(pending) = self.changes.get(event_id) else {
            return;
        };
        if pending.phase != PinPhase::Requested(request) {
            tracing::debug!(
                request,
                event_id,
                "dropping a pin change a later one replaced"
            );
            return;
        }
        let pins = pending.pins();
        let echoed =
            self.room_id.as_ref() != Some(&pending.room_id) || self.loaded(event_id) == pins;
        match outcome {
            ActionOutcome::Done if echoed => {
                self.changes.remove(event_id);
            }
            ActionOutcome::Done => {
                if let Some(pending) = self.changes.get_mut(event_id) {
                    pending.phase = PinPhase::AwaitingEcho;
                }
            }
            ActionOutcome::Failed => {
                self.changes.remove(event_id);
                let kind = if pins {
                    UserMessageKind::MessagePinFailed
                } else {
                    UserMessageKind::MessageUnpinFailed
                };
                show_toast(self.output.as_ref(), Toast::Error(UserMessage::new(kind)));
            }
        }
        self.publish();
    }

    pub(super) fn clear(&mut self) {
        self.tasks.cancel_and_detach();
        self.watch = self.watch.wrapping_add(1);
        self.forget();
        self.keep_requested_changes();
        self.publish();
    }

    pub(super) async fn restart(&mut self) {
        tokio::join!(self.tasks.restart(), self.pins.restart());
        self.watch = self.watch.wrapping_add(1);
        self.forget();
        self.changes.clear();
    }

    pub(super) async fn shutdown(&mut self) {
        tokio::join!(self.tasks.shutdown(), self.pins.shutdown());
    }

    fn keep_requested_changes(&mut self) {
        self.changes
            .retain(|_, pending| matches!(pending.phase, PinPhase::Requested(_)));
    }

    fn settle_echoed_changes(&mut self) {
        let Some(room_id) = self.room_id.clone() else {
            return;
        };
        let messages = Arc::clone(&self.messages);
        self.changes.retain(|event_id, pending| {
            let echoed = pending.room_id == room_id
                && messages.iter().any(|message| &message.event_id == event_id) == pending.pins();
            !(pending.phase == PinPhase::AwaitingEcho && echoed)
        });
    }

    fn loaded(&self, event_id: &str) -> bool {
        self.position_of(event_id).is_some()
    }

    fn shows_pinned(&self, room_id: &RoomId, event_id: &str) -> bool {
        match self.changes.get(event_id) {
            Some(pending) if &pending.room_id == room_id => pending.pins(),
            Some(_) | None => self.room_id.as_ref() == Some(room_id) && self.loaded(event_id),
        }
    }

    fn pinned_ids(&self) -> BTreeSet<String> {
        let mut ids: BTreeSet<String> = self
            .messages
            .iter()
            .map(|message| message.event_id.clone())
            .collect();
        for (event_id, pending) in &self.changes {
            if self.room_id.as_ref() != Some(&pending.room_id) {
                continue;
            }
            if pending.pins() {
                ids.insert(event_id.clone());
            } else {
                ids.remove(event_id);
            }
        }
        ids
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
            pinned_ids: Arc::new(self.pinned_ids()),
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
