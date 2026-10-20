use std::collections::HashSet;
use std::sync::Arc;

use super::event::{AppEvent, DepartureEvent};
use super::input::EventSender;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{LeaveSpaceView, SpaceToLeave, Toast};
use crate::domain::departure::{AdminStanding, SpaceDeparture};
use crate::domain::room::{RoomId, Space};
use crate::ports::matrix::LeavePort;
use crate::ports::output::AppOutputPort;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SpaceLeaveOutcome {
    Left,
    RoomsKept,
    Failed,
}

struct OpenDialog {
    generation: u64,
    space_id: RoomId,
    name: String,
    departure: SpaceDeparture,
    standing: Option<AdminStanding>,
    error: UserMessage,
}

pub(super) struct SpaceLeave {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    tasks: TaskGroup,
    actions: TaskGroup,
    generation: u64,
    open: Option<OpenDialog>,
    leaving: HashSet<RoomId>,
}

impl SpaceLeave {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            tasks: TaskGroup::new("space-leave"),
            actions: TaskGroup::new("space-leaves"),
            generation: 0,
            open: None,
            leaving: HashSet::new(),
        }
    }

    pub(super) fn open(
        &mut self,
        port: Arc<dyn LeavePort>,
        space: Option<&Space>,
        departure: SpaceDeparture,
    ) {
        let Some(space) = space else {
            tracing::debug!("the space is not joined, not offering to leave it");
            return;
        };
        if self.shows(&space.id) {
            return;
        }
        self.tasks.cancel_and_detach();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let space_id = RoomId::new(space.id.clone());
        tracing::debug!(
            space = %space_id,
            rooms = departure.rooms.len(),
            spaces = departure.spaces.len(),
            "offering to leave the space"
        );
        self.open = Some(OpenDialog {
            generation,
            space_id: space_id.clone(),
            name: space.name.clone(),
            departure,
            standing: None,
            error: UserMessage::default(),
        });
        self.publish();

        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let standing = tokio::select! {
                () = cancel.cancelled() => return,
                standing = port.admin_standing(&space_id) => standing
                    .inspect_err(|e| tracing::debug!(space = %space_id, "could not tell who else manages the space: {e}"))
                    .ok(),
            };
            let read = DepartureEvent::StandingRead {
                generation,
                standing,
            };
            drop(events.send(AppEvent::Departure(read)));
        });
    }

    pub(super) fn close(&mut self) {
        if self.open.take().is_none() {
            return;
        }
        self.tasks.cancel_and_detach();
        self.publish();
    }

    pub(super) fn standing_read(&mut self, generation: u64, standing: Option<AdminStanding>) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|open| open.generation == generation)
        else {
            tracing::debug!(generation, "dropping a superseded admin check");
            return;
        };
        open.standing = standing;
        self.publish();
    }

    pub(super) fn follow(&mut self, space: Option<&Space>, departure: SpaceDeparture) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if self.leaving.contains(&open.space_id) {
            return;
        }
        let Some(space) = space else {
            tracing::debug!(space = %open.space_id, "the space the dialog offers to leave is gone");
            self.close();
            return;
        };
        if open.name == space.name && open.departure == departure {
            return;
        }
        open.name.clone_from(&space.name);
        open.departure = departure;
        self.publish();
    }

    pub(super) fn leave(&mut self, port: Arc<dyn LeavePort>, space_id: &RoomId, with_rooms: bool) {
        let Some(open) = self.open.as_mut().filter(|open| open.space_id == *space_id) else {
            tracing::debug!(space = %space_id, "the dialog does not offer that space, not leaving it");
            return;
        };
        if !self.leaving.insert(space_id.clone()) {
            tracing::debug!(space = %space_id, "the space is already being left");
            return;
        }
        open.error = UserMessage::default();
        let mut first = Vec::new();
        if with_rooms {
            first.extend(open.departure.rooms.iter().cloned());
            first.extend(open.departure.spaces.iter().cloned());
        }
        let name = open.name.clone();
        tracing::info!(space = %space_id, also = first.len(), "leaving the space");
        self.publish();

        let space_id = space_id.clone();
        let events = self.events.clone();
        let cancel = self.actions.token();
        self.actions.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                outcome = leave_in_order(port.as_ref(), &first, &space_id) => outcome,
            };
            let left = DepartureEvent::SpaceLeft {
                space_id,
                name,
                outcome,
            };
            drop(events.send(AppEvent::Departure(left)));
        });
    }

    pub(super) fn left(&mut self, space_id: &RoomId, name: &str, outcome: SpaceLeaveOutcome) {
        if !self.leaving.remove(space_id) {
            tracing::debug!(space = %space_id, "dropping a space leave nobody is waiting for");
            return;
        }
        match outcome {
            SpaceLeaveOutcome::Left => {
                if self.shows(space_id) {
                    self.close();
                    return;
                }
            }
            SpaceLeaveOutcome::RoomsKept => {
                if self.shows(space_id) {
                    self.open = None;
                    self.tasks.cancel_and_detach();
                }
                show_toast(
                    self.output.as_ref(),
                    Toast::Error(UserMessage::about(UserMessageKind::SpaceRoomsKept, &name)),
                );
            }
            SpaceLeaveOutcome::Failed => {
                let message = UserMessage::about(UserMessageKind::LeaveRoomFailed, &name);
                match self.open.as_mut().filter(|open| open.space_id == *space_id) {
                    Some(open) => open.error = message,
                    None => show_toast(self.output.as_ref(), Toast::Error(message)),
                }
            }
        }
        self.publish();
    }

    pub(super) async fn restart(&mut self) {
        tokio::join!(self.tasks.restart(), self.actions.restart());
        self.open = None;
        self.leaving.clear();
    }

    pub(super) async fn shutdown(&mut self) {
        tokio::join!(self.tasks.shutdown(), self.actions.shutdown());
    }

    pub(super) fn shown_space(&self) -> Option<RoomId> {
        self.open.as_ref().map(|open| open.space_id.clone())
    }

    pub(super) fn departing_rooms(&self, space_id: &RoomId) -> Vec<RoomId> {
        self.open
            .as_ref()
            .filter(|open| open.space_id == *space_id && !self.leaving.contains(space_id))
            .map(|open| open.departure.rooms.clone())
            .unwrap_or_default()
    }

    fn shows(&self, space_id: &str) -> bool {
        self.open
            .as_ref()
            .is_some_and(|open| open.space_id.as_ref() == space_id)
    }

    fn publish(&self) {
        let view = self
            .open
            .as_ref()
            .map_or_else(LeaveSpaceView::default, |open| LeaveSpaceView {
                space: Some(SpaceToLeave {
                    id: open.space_id.clone(),
                    name: open.name.clone(),
                    rooms: open.departure.rooms.len(),
                    spaces: open.departure.spaces.len(),
                }),
                standing: open.standing,
                leaving: self.leaving.contains(&open.space_id),
                error: open.error.clone(),
            });
        self.output
            .publish(Box::new(move |state| state.leave_space = view));
    }
}

async fn leave_in_order(
    port: &dyn LeavePort,
    first: &[RoomId],
    space_id: &RoomId,
) -> SpaceLeaveOutcome {
    let mut kept = 0_usize;
    for room_id in first {
        if let Err(e) = port.leave(room_id).await {
            tracing::warn!(%room_id, space = %space_id, "failed to leave a room in the space: {e}");
            kept += 1;
        }
    }
    if let Err(e) = port.leave(space_id).await {
        tracing::warn!(space = %space_id, "failed to leave the space: {e}");
        return SpaceLeaveOutcome::Failed;
    }
    if kept > 0 {
        return SpaceLeaveOutcome::RoomsKept;
    }
    SpaceLeaveOutcome::Left
}
