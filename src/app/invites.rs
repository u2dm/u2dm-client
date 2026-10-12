use std::collections::HashMap;
use std::sync::Arc;

use super::event::AppEvent;
use super::input::EventSender;
use super::room_directory::Membership;
use super::room_info::ActionOutcome;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{InviteView, PendingAnswer, Toast};
use crate::domain::invite::Invite;
use crate::domain::room::RoomId;
use crate::error::Result;
use crate::ports::matrix::InvitePort;
use crate::ports::output::AppOutputPort;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Answer {
    Accept,
    Decline,
}

impl Answer {
    async fn send(self, port: &dyn InvitePort, room_id: &RoomId) -> Result<()> {
        match self {
            Self::Accept => port.accept(room_id).await,
            Self::Decline => port.decline(room_id).await,
        }
    }

    fn pending(self) -> PendingAnswer {
        match self {
            Self::Accept => PendingAnswer::Accept,
            Self::Decline => PendingAnswer::Decline,
        }
    }

    fn failure(self) -> UserMessageKind {
        match self {
            Self::Accept => UserMessageKind::JoinRoomFailed,
            Self::Decline => UserMessageKind::DeclineInviteFailed,
        }
    }
}

enum Stage {
    Answering(Answer),
    AwaitingList,
}

impl Stage {
    fn pending(&self) -> PendingAnswer {
        match self {
            Self::Answering(answer) => answer.pending(),
            Self::AwaitingList => PendingAnswer::Accept,
        }
    }
}

pub(super) enum Arrival {
    Room(RoomId),
    Space(RoomId),
}

fn arrival_of(invite: &Invite, membership: &Membership<'_>) -> Option<Arrival> {
    let id = invite.id.clone();
    if invite.is_space() {
        return membership.has_space(&id).then_some(Arrival::Space(id));
    }
    membership.has_room(&id).then_some(Arrival::Room(id))
}

struct OpenSheet {
    invite: Arc<Invite>,
    error: UserMessage,
}

pub(super) struct Invites {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    actions: TaskGroup,
    open: Option<OpenSheet>,
    answering: HashMap<RoomId, Stage>,
}

impl Invites {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            actions: TaskGroup::new("invite-answers"),
            open: None,
            answering: HashMap::new(),
        }
    }

    pub(super) fn open(&mut self, invite: Option<&Arc<Invite>>) {
        let Some(invite) = invite else {
            tracing::debug!("the invite is not listed, not opening it");
            return;
        };
        if self.shows(&invite.id) {
            return;
        }
        self.forget_awaited_arrivals();
        tracing::debug!(room_id = %invite.id, "opening the invite");
        self.open = Some(OpenSheet {
            invite: Arc::clone(invite),
            error: UserMessage::default(),
        });
        self.publish();
    }

    pub(super) fn close(&mut self) {
        if self.open.take().is_none() {
            return;
        }
        self.forget_awaited_arrivals();
        self.publish();
    }

    pub(super) fn answer(
        &mut self,
        port: Arc<dyn InvitePort>,
        invite: Option<&Arc<Invite>>,
        answer: Answer,
    ) {
        let Some(invite) = invite else {
            tracing::debug!(?answer, "the invite is not listed, not answering it");
            return;
        };
        let room_id = invite.id.clone();
        if self.answering.contains_key(&room_id) {
            tracing::debug!(%room_id, ?answer, "the invite is already being answered");
            return;
        }
        self.answering
            .insert(room_id.clone(), Stage::Answering(answer));
        self.clear_error(&room_id);
        self.publish();

        let name = invite.name.clone();
        let events = self.events.clone();
        let cancel = self.actions.token();
        self.actions.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                answered = answer.send(port.as_ref(), &room_id) => match answered {
                    Ok(()) => ActionOutcome::Done,
                    Err(e) => {
                        tracing::warn!(%room_id, ?answer, "failed to answer the invite: {e}");
                        ActionOutcome::Failed
                    }
                },
            };
            drop(events.send(AppEvent::InviteAnswered {
                room_id,
                name,
                answer,
                outcome,
            }));
        });
    }

    pub(super) fn answered(
        &mut self,
        room_id: &RoomId,
        name: &str,
        answer: Answer,
        outcome: ActionOutcome,
        membership: &Membership<'_>,
    ) -> Option<Arrival> {
        if !matches!(self.answering.get(room_id), Some(Stage::Answering(pending)) if *pending == answer)
        {
            tracing::debug!(%room_id, ?answer, "dropping an answer nobody is waiting for");
            return None;
        }
        self.answering.remove(room_id);
        let shown = self.shows(room_id);
        match (answer, outcome) {
            (Answer::Accept, ActionOutcome::Done) if shown => {
                let arrival = self
                    .open
                    .as_ref()
                    .and_then(|open| arrival_of(&open.invite, membership));
                if arrival.is_some() {
                    self.open = None;
                } else {
                    self.answering.insert(room_id.clone(), Stage::AwaitingList);
                }
                self.publish();
                arrival
            }
            (Answer::Accept, ActionOutcome::Done) => {
                tracing::debug!(%room_id, "accepted an invite whose sheet has closed");
                None
            }
            (Answer::Decline, ActionOutcome::Done) => {
                if shown {
                    self.open = None;
                    self.publish();
                }
                None
            }
            (answer, ActionOutcome::Failed) => {
                self.report(room_id, UserMessage::about(answer.failure(), &name));
                self.publish();
                None
            }
        }
    }

    pub(super) fn follow(
        &mut self,
        invites: &[Arc<Invite>],
        membership: &Membership<'_>,
    ) -> Option<Arrival> {
        let open = self.open.as_mut()?;
        let room_id = open.invite.id.clone();
        match self.answering.get(&room_id) {
            Some(Stage::AwaitingList) => {
                let arrival = arrival_of(&open.invite, membership)?;
                self.answering.remove(&room_id);
                self.open = None;
                self.publish();
                return Some(arrival);
            }
            Some(Stage::Answering(Answer::Accept)) => return None,
            Some(Stage::Answering(Answer::Decline)) | None => {}
        }
        match invites.iter().find(|invite| invite.id == room_id) {
            Some(listed) if Arc::ptr_eq(listed, &open.invite) || **listed == *open.invite => {}
            Some(listed) => {
                open.invite = Arc::clone(listed);
                self.publish();
            }
            None => {
                tracing::debug!(%room_id, "the invite the sheet shows is gone");
                self.open = None;
                self.publish();
            }
        }
        None
    }

    pub(super) async fn restart(&mut self) {
        self.actions.restart().await;
        self.open = None;
        self.answering.clear();
    }

    pub(super) async fn shutdown(&mut self) {
        self.actions.shutdown().await;
    }

    fn shows(&self, room_id: &RoomId) -> bool {
        self.open
            .as_ref()
            .is_some_and(|open| open.invite.id == *room_id)
    }

    fn forget_awaited_arrivals(&mut self) {
        self.answering
            .retain(|_, stage| matches!(stage, Stage::Answering(_)));
    }

    fn clear_error(&mut self, room_id: &RoomId) {
        if let Some(open) = self.open.as_mut().filter(|open| open.invite.id == *room_id) {
            open.error = UserMessage::default();
        }
    }

    fn report(&mut self, room_id: &RoomId, message: UserMessage) {
        match self.open.as_mut().filter(|open| open.invite.id == *room_id) {
            Some(open) => open.error = message,
            None => show_toast(self.output.as_ref(), Toast::Error(message)),
        }
    }

    fn publish(&self) {
        let view = self
            .open
            .as_ref()
            .map_or_else(InviteView::default, |open| InviteView {
                invite: Some(Arc::clone(&open.invite)),
                answer: self
                    .answering
                    .get(&open.invite.id)
                    .map_or(PendingAnswer::None, Stage::pending),
                error: open.error.clone(),
            });
        self.output
            .publish(Box::new(move |state| state.invite = view));
    }
}
