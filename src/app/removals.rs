use std::collections::HashSet;
use std::sync::Arc;

use super::event::{AppEvent, DepartureEvent};
use super::input::EventSender;
use super::room_info::ActionOutcome;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{RemovalView, Toast};
use crate::domain::departure::Removal;
use crate::domain::room::RoomId;
use crate::ports::matrix::LeavePort;
use crate::ports::output::AppOutputPort;

struct OpenSheet {
    removal: Arc<Removal>,
    error: UserMessage,
}

pub(super) struct Removals {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    actions: TaskGroup,
    open: Option<OpenSheet>,
    forgetting: HashSet<RoomId>,
}

impl Removals {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            actions: TaskGroup::new("removal-forgets"),
            open: None,
            forgetting: HashSet::new(),
        }
    }

    pub(super) fn open(&mut self, removal: Option<&Arc<Removal>>) {
        let Some(removal) = removal else {
            tracing::debug!("the removal is not listed, not opening it");
            return;
        };
        if self.shows(&removal.id) {
            return;
        }
        tracing::debug!(room_id = %removal.id, "opening the removal");
        self.open = Some(OpenSheet {
            removal: Arc::clone(removal),
            error: UserMessage::default(),
        });
        self.publish();
    }

    pub(super) fn close(&mut self) {
        if self.open.take().is_some() {
            self.publish();
        }
    }

    pub(super) fn forget(&mut self, port: Arc<dyn LeavePort>, removal: Option<&Arc<Removal>>) {
        let Some(removal) = removal else {
            tracing::debug!("the removal is not listed, not forgetting it");
            return;
        };
        let room_id = removal.id.clone();
        if !self.forgetting.insert(room_id.clone()) {
            tracing::debug!(%room_id, "the room is already being forgotten");
            return;
        }
        self.clear_error(&room_id);
        self.publish();

        let name = removal.name.clone();
        let events = self.events.clone();
        let cancel = self.actions.token();
        self.actions.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                forgot = port.forget(&room_id) => match forgot {
                    Ok(()) => ActionOutcome::Done,
                    Err(e) => {
                        tracing::warn!(%room_id, "failed to forget the room: {e}");
                        ActionOutcome::Failed
                    }
                },
            };
            let forgotten = DepartureEvent::RemovalForgotten {
                room_id,
                name,
                outcome,
            };
            drop(events.send(AppEvent::Departure(forgotten)));
        });
    }

    pub(super) fn forgotten(
        &mut self,
        room_id: &RoomId,
        name: &str,
        outcome: ActionOutcome,
    ) -> bool {
        if !self.forgetting.remove(room_id) {
            tracing::debug!(%room_id, "dropping a forget nobody is waiting for");
            return false;
        }
        match outcome {
            ActionOutcome::Done => {
                if self.shows(room_id) {
                    self.open = None;
                }
            }
            ActionOutcome::Failed => {
                self.report(
                    room_id,
                    UserMessage::about(UserMessageKind::ForgetRoomFailed, &name),
                );
            }
        }
        self.publish();
        outcome == ActionOutcome::Done
    }

    pub(super) fn follow(&mut self, removals: &[Arc<Removal>]) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let room_id = open.removal.id.clone();
        if self.forgetting.contains(&room_id) {
            return;
        }
        match removals.iter().find(|removal| removal.id == room_id) {
            Some(listed) if Arc::ptr_eq(listed, &open.removal) || **listed == *open.removal => {}
            Some(listed) => {
                open.removal = Arc::clone(listed);
                self.publish();
            }
            None => {
                tracing::debug!(%room_id, "the removal the sheet shows is gone");
                self.open = None;
                self.publish();
            }
        }
    }

    pub(super) async fn restart(&mut self) {
        self.actions.restart().await;
        self.open = None;
        self.forgetting.clear();
    }

    pub(super) async fn shutdown(&mut self) {
        self.actions.shutdown().await;
    }

    fn shows(&self, room_id: &RoomId) -> bool {
        self.open
            .as_ref()
            .is_some_and(|open| open.removal.id == *room_id)
    }

    fn clear_error(&mut self, room_id: &RoomId) {
        if let Some(open) = self
            .open
            .as_mut()
            .filter(|open| open.removal.id == *room_id)
        {
            open.error = UserMessage::default();
        }
    }

    fn report(&mut self, room_id: &RoomId, message: UserMessage) {
        match self
            .open
            .as_mut()
            .filter(|open| open.removal.id == *room_id)
        {
            Some(open) => open.error = message,
            None => show_toast(self.output.as_ref(), Toast::Error(message)),
        }
    }

    fn publish(&self) {
        let view = self
            .open
            .as_ref()
            .map_or_else(RemovalView::default, |open| RemovalView {
                removal: Some(Arc::clone(&open.removal)),
                forgetting: self.forgetting.contains(&open.removal.id),
                error: open.error.clone(),
            });
        self.output
            .publish(Box::new(move |state| state.removal = view));
    }
}
