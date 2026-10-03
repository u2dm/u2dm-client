use std::future::Future;
use std::sync::Arc;

use super::event::{AppEvent, Enqueue};
use super::input::EventSender;
use super::send_lanes::SendLanes;
use crate::commands::ui::{Draft, MessageDraft};
use crate::commands::view::UnsentMessage;
use crate::domain::message::MessageEdit;
use crate::domain::room::RoomId;
use crate::error::Result;
use crate::ports::matrix::TimelinePort;
use crate::ports::output::AppOutputPort;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Enqueueing,
    Refused,
}

struct Submission {
    message: UnsentMessage,
    stage: Stage,
}

impl Submission {
    fn is(&self, submission: i32, stage: Stage) -> bool {
        self.message.submission == submission && self.stage == stage
    }
}

pub(super) struct Submissions {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    issued: i32,
    held: Vec<Submission>,
}

impl Submissions {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            issued: 0,
            held: Vec::new(),
        }
    }

    pub(super) fn send(
        &mut self,
        lanes: &mut SendLanes,
        timeline: Arc<dyn TimelinePort>,
        room_id: RoomId,
        draft: MessageDraft,
    ) {
        let body = draft.body.clone();
        let reply_to = draft.reply.as_ref().map(|reply| reply.event_id.clone());
        let submission = self.hold(&room_id, Draft::Message(draft), Stage::Enqueueing);
        let lane_room = room_id.clone();
        self.settle_on_lane(lanes, room_id, submission, async move {
            match reply_to {
                Some(event_id) => timeline.send_reply(&lane_room, &body, &event_id).await,
                None => timeline.send_text(&lane_room, &body).await,
            }
        });
    }

    pub(super) fn edit(
        &mut self,
        lanes: &mut SendLanes,
        timeline: Arc<dyn TimelinePort>,
        room_id: RoomId,
        edit: MessageEdit,
    ) {
        let queued = edit.clone();
        let submission = self.hold(&room_id, Draft::Edit(edit), Stage::Enqueueing);
        let lane_room = room_id.clone();
        self.settle_on_lane(lanes, room_id, submission, async move {
            timeline.edit_message(&lane_room, &queued).await
        });
    }

    pub(super) fn unsaved(
        &mut self,
        room_id: &RoomId,
        edit: MessageEdit,
        selected: Option<&RoomId>,
    ) {
        self.hold(room_id, Draft::Edit(edit), Stage::Refused);
        self.offer(selected);
    }

    fn hold(&mut self, room_id: &RoomId, draft: Draft, stage: Stage) -> i32 {
        self.issued = self.issued.wrapping_add(1);
        self.held.push(Submission {
            message: UnsentMessage {
                submission: self.issued,
                room_id: room_id.clone(),
                draft,
            },
            stage,
        });
        self.issued
    }

    fn settle_on_lane(
        &self,
        lanes: &mut SendLanes,
        room_id: RoomId,
        submission: i32,
        enqueue: impl Future<Output = Result<()>> + Send + 'static,
    ) {
        let events = self.events.clone();
        lanes.spawn(room_id, async move {
            let enqueue = match enqueue.await {
                Ok(()) => Enqueue::Accepted,
                Err(e) => {
                    tracing::warn!("failed to enqueue a submission: {e}");
                    Enqueue::Refused
                }
            };
            drop(events.send(AppEvent::SubmissionSettled {
                submission,
                enqueue,
            }));
        });
    }

    pub(super) fn settled(
        &mut self,
        submission: i32,
        enqueue: Enqueue,
        selected: Option<&RoomId>,
    ) {
        let Some(held) = self
            .held
            .iter_mut()
            .find(|held| held.is(submission, Stage::Enqueueing))
        else {
            tracing::debug!(submission, "dropping the outcome of a forgotten submission");
            return;
        };
        match enqueue {
            Enqueue::Accepted => self
                .held
                .retain(|held| held.message.submission != submission),
            Enqueue::Refused => {
                held.stage = Stage::Refused;
                self.offer(selected);
            }
        }
    }

    pub(super) fn dismiss(&mut self, submission: i32, selected: Option<&RoomId>) {
        self.held
            .retain(|held| !held.is(submission, Stage::Refused));
        self.offer(selected);
    }

    pub(super) fn offer(&self, selected: Option<&RoomId>) {
        let unsent = self
            .held
            .iter()
            .find(|held| held.stage == Stage::Refused && Some(&held.message.room_id) == selected)
            .map(|held| held.message.clone());
        self.output
            .publish(Box::new(move |state| state.unsent = unsent));
    }

    pub(super) fn forget_all(&mut self) {
        self.held.clear();
    }
}
