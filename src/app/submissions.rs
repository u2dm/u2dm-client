use std::sync::Arc;

use super::event::{AppEvent, Enqueue};
use super::input::EventSender;
use super::send_lanes::SendLanes;
use crate::commands::ui::MessageDraft;
use crate::commands::view::UnsentMessage;
use crate::domain::room::RoomId;
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
        self.issued = self.issued.wrapping_add(1);
        let submission = self.issued;
        let body = draft.body.clone();
        let reply_to = draft.reply.as_ref().map(|reply| reply.event_id.clone());
        self.held.push(Submission {
            message: UnsentMessage {
                submission,
                room_id: room_id.clone(),
                draft,
            },
            stage: Stage::Enqueueing,
        });
        let events = self.events.clone();
        lanes.spawn(room_id.clone(), async move {
            let enqueued = match reply_to {
                Some(event_id) => timeline.send_reply(&room_id, &body, &event_id).await,
                None => timeline.send_text(&room_id, &body).await,
            };
            let enqueue = match enqueued {
                Ok(()) => Enqueue::Accepted,
                Err(e) => {
                    tracing::warn!("failed to enqueue message: {e}");
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
