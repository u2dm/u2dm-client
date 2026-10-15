use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;

use super::event::{AppEvent, InvitePeopleEvent};
use super::input::EventSender;
use super::room_directory::Membership;
use super::show_toast;
use super::space_index::AVATAR_BATCH;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{
    InvitePeopleView, InviteTarget, InviteeRow, InviteeState, PeopleSearchStatus, Toast,
};
use crate::domain::invite_people::{
    InviteFailure, InviteResult, PeopleSearchFailure, Person, SearchResult,
};
use crate::domain::room::RoomId;
use crate::domain::user_info::{RoomMembership, UserId};
use crate::ports::matrix::InvitePeoplePort;
use crate::ports::output::AppOutputPort;

const PEOPLE_SEARCH_PAUSE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Search {
    Idle,
    Searching,
    Ready,
    Failed(PeopleSearchFailure),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PickStage {
    Waiting,
    Sending,
    Failed(InviteFailure),
}

struct Picked {
    person: Arc<Person>,
    stage: PickStage,
}

struct OpenDialog {
    target: InviteTarget,
    query: String,
    search: Search,
    results: Vec<Arc<Person>>,
    limited: bool,
    picked: Vec<Picked>,
    batch: Option<u64>,
    avatars_ready: usize,
}

impl OpenDialog {
    fn new(target: InviteTarget) -> Self {
        Self {
            target,
            query: String::new(),
            search: Search::Idle,
            results: Vec::new(),
            limited: false,
            picked: Vec::new(),
            batch: None,
            avatars_ready: 0,
        }
    }

    fn picked(&self, user_id: &UserId) -> Option<&Picked> {
        self.picked
            .iter()
            .find(|picked| picked.person.user_id == *user_id)
    }

    fn settle(&mut self, user_id: &UserId, outcome: InviteResult) {
        match outcome {
            Ok(()) => {
                self.picked
                    .retain(|picked| picked.person.user_id != *user_id);
                for person in self
                    .results
                    .iter_mut()
                    .filter(|person| person.user_id == *user_id)
                {
                    *person = Arc::new(Person {
                        membership: RoomMembership::Invited,
                        ..(**person).clone()
                    });
                }
            }
            Err(failure) => {
                if let Some(picked) = self
                    .picked
                    .iter_mut()
                    .find(|picked| picked.person.user_id == *user_id)
                {
                    picked.stage = PickStage::Failed(failure);
                }
            }
        }
    }

    fn shown_avatars(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.picked
            .iter()
            .map(|picked| &picked.person)
            .chain(&self.results)
            .filter_map(|person| person.avatar_mxc.clone())
            .filter(|mxc| seen.insert(mxc.clone()))
            .collect()
    }
}

struct Batch {
    people: Vec<Arc<Person>>,
    room_id: RoomId,
}

pub(super) struct InvitePeople {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    tasks: TaskGroup,
    actions: TaskGroup,
    generation: u64,
    next_batch: u64,
    open: Option<OpenDialog>,
    batches: HashMap<u64, Batch>,
}

impl InvitePeople {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            tasks: TaskGroup::new("people-search"),
            actions: TaskGroup::new("invite-sends"),
            generation: 0,
            next_batch: 0,
            open: None,
            batches: HashMap::new(),
        }
    }

    pub(super) fn open(&mut self, target: Option<InviteTarget>) {
        let Some(target) = target else {
            tracing::debug!("nobody can be invited there, not opening the invite dialog");
            return;
        };
        if self.target() == Some(&target.room_id) {
            return;
        }
        tracing::debug!(room_id = %target.room_id, "opening the invite dialog");
        self.restart_search();
        self.open = Some(OpenDialog::new(target));
        self.publish();
    }

    pub(super) fn close(&mut self) {
        if self.open.take().is_none() {
            return;
        }
        self.restart_search();
        self.publish();
    }

    pub(super) fn target(&self) -> Option<&RoomId> {
        self.open.as_ref().map(|open| &open.target.room_id)
    }

    pub(super) fn search(&mut self, port: Arc<dyn InvitePeoplePort>, term: &str) {
        let term = term.trim();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if open.query == term {
            return;
        }
        term.clone_into(&mut open.query);
        self.restart_search();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if term.is_empty() {
            open.search = Search::Idle;
            open.results.clear();
            open.limited = false;
        } else {
            open.search = Search::Searching;
            let room_id = open.target.room_id.clone();
            self.spawn_search(port, room_id, term.to_owned(), PEOPLE_SEARCH_PAUSE);
        }
        self.publish();
    }

    pub(super) fn retry(&mut self, port: Arc<dyn InvitePeoplePort>) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|open| matches!(open.search, Search::Failed(_)))
        else {
            return;
        };
        open.search = Search::Searching;
        let room_id = open.target.room_id.clone();
        let term = open.query.clone();
        self.restart_search();
        self.spawn_search(port, room_id, term, Duration::ZERO);
        self.publish();
    }

    pub(super) fn found(
        &mut self,
        port: Arc<dyn InvitePeoplePort>,
        generation: u64,
        outcome: SearchResult,
    ) {
        if generation != self.generation {
            tracing::debug!("dropping people found for an old search");
            return;
        }
        let Some(open) = self.open.as_mut() else {
            return;
        };
        match outcome {
            Ok(found) => {
                open.search = Search::Ready;
                open.results = found.people.into_iter().map(Arc::new).collect();
                open.limited = found.limited;
            }
            Err(failure) => {
                open.search = Search::Failed(failure);
                open.results.clear();
                open.limited = false;
            }
        }
        let mxcs = open.shown_avatars();
        if !mxcs.is_empty() {
            self.spawn_avatars(port, mxcs);
        }
        self.publish();
    }

    pub(super) fn avatars_landed(&mut self, generation: u64, ready: usize) {
        if generation != self.generation || ready == 0 {
            return;
        }
        if let Some(open) = self.open.as_mut() {
            open.avatars_ready = open.avatars_ready.wrapping_add(ready);
            self.publish();
        }
    }

    pub(super) fn pick(&mut self, user_id: &UserId) {
        let Some(open) = self.open.as_mut().filter(|open| open.batch.is_none()) else {
            return;
        };
        if open.picked(user_id).is_some() {
            return;
        }
        let Some(person) = open
            .results
            .iter()
            .find(|person| person.user_id == *user_id && person.invitable())
        else {
            tracing::debug!(%user_id, "that person cannot be picked");
            return;
        };
        open.picked.push(Picked {
            person: Arc::clone(person),
            stage: PickStage::Waiting,
        });
        self.publish();
    }

    pub(super) fn unpick(&mut self, user_id: &UserId) {
        let Some(open) = self.open.as_mut().filter(|open| open.batch.is_none()) else {
            return;
        };
        let before = open.picked.len();
        open.picked
            .retain(|picked| picked.person.user_id != *user_id);
        if open.picked.len() != before {
            self.publish();
        }
    }

    pub(super) fn send(&mut self, port: Arc<dyn InvitePeoplePort>, reason: &str) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|open| open.batch.is_none() && !open.picked.is_empty())
        else {
            tracing::debug!("nobody is waiting to be invited");
            return;
        };
        let batch = self.next_batch;
        self.next_batch = self.next_batch.wrapping_add(1);
        for picked in &mut open.picked {
            picked.stage = PickStage::Sending;
        }
        open.batch = Some(batch);
        let people: Vec<Arc<Person>> = open
            .picked
            .iter()
            .map(|picked| Arc::clone(&picked.person))
            .collect();
        let room_id = open.target.room_id.clone();
        let reason = Some(reason.trim())
            .filter(|reason| !reason.is_empty())
            .map(str::to_owned);
        tracing::info!(%room_id, people = people.len(), "inviting people");
        self.batches.insert(
            batch,
            Batch {
                people: people.clone(),
                room_id: room_id.clone(),
            },
        );
        self.publish();

        let events = self.events.clone();
        let cancel = self.actions.token();
        self.actions.spawn(async move {
            let mut outcomes = Vec::with_capacity(people.len());
            let mut throttled = false;
            for person in people {
                let user_id = person.user_id.clone();
                if throttled {
                    outcomes.push((user_id, Err(InviteFailure::RateLimited)));
                    continue;
                }
                let outcome = tokio::select! {
                    () = cancel.cancelled() => return,
                    outcome = port.invite(&room_id, &user_id, reason.as_deref()) => outcome,
                };
                throttled = outcome == Err(InviteFailure::RateLimited);
                outcomes.push((user_id, outcome));
            }
            drop(events.send(AppEvent::InvitePeople(InvitePeopleEvent::Sent {
                batch,
                outcomes,
            })));
        });
    }

    pub(super) fn sent(
        &mut self,
        batch: u64,
        outcomes: Vec<(UserId, InviteResult)>,
    ) -> Option<RoomId> {
        let Some(sent) = self.batches.remove(&batch) else {
            tracing::debug!(batch, "dropping invites nobody is waiting for");
            return None;
        };
        let invited_any = outcomes.iter().any(|(_, outcome)| outcome.is_ok());
        match self.open.as_mut().filter(|open| open.batch == Some(batch)) {
            Some(open) => {
                open.batch = None;
                for (user_id, outcome) in outcomes {
                    open.settle(&user_id, outcome);
                }
                if open.picked.is_empty() {
                    tracing::info!(room_id = %sent.room_id, "invited everyone, closing the dialog");
                    self.close();
                } else {
                    self.publish();
                }
            }
            None => report_after_close(self.output.as_ref(), &sent, &outcomes),
        }
        invited_any.then_some(sent.room_id)
    }

    pub(super) fn follow(&mut self, membership: &Membership<'_>) {
        if self
            .target()
            .is_some_and(|room_id| !membership.has_joined(room_id))
        {
            tracing::debug!("the invite dialog's room is gone, closing it");
            self.close();
        }
    }

    pub(super) async fn restart(&mut self) {
        tokio::join!(self.tasks.restart(), self.actions.restart());
        self.open = None;
        self.batches.clear();
    }

    pub(super) async fn shutdown(&mut self) {
        tokio::join!(self.tasks.shutdown(), self.actions.shutdown());
    }

    fn restart_search(&mut self) {
        self.tasks.cancel_and_detach();
        self.generation = self.generation.wrapping_add(1);
    }

    fn spawn_search(
        &mut self,
        port: Arc<dyn InvitePeoplePort>,
        room_id: RoomId,
        term: String,
        pause: Duration,
    ) {
        let generation = self.generation;
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                outcome = async {
                    sleep(pause).await;
                    port.search_people(&room_id, &term).await
                } => outcome,
            };
            drop(
                events.send(AppEvent::InvitePeople(InvitePeopleEvent::Found {
                    generation,
                    outcome,
                })),
            );
        });
    }

    fn spawn_avatars(&mut self, port: Arc<dyn InvitePeoplePort>, mxcs: Vec<String>) {
        let generation = self.generation;
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            for batch in mxcs.chunks(AVATAR_BATCH) {
                let ready = tokio::select! {
                    () = cancel.cancelled() => return,
                    ready = port.fetch_avatars(batch) => ready,
                };
                if ready > 0 {
                    drop(
                        events.send(AppEvent::InvitePeople(InvitePeopleEvent::AvatarsReady {
                            generation,
                            ready,
                        })),
                    );
                }
            }
        });
    }

    fn publish(&self) {
        let view = self.view();
        self.output
            .publish(Box::new(move |state| state.invite_people = view));
    }

    fn view(&self) -> InvitePeopleView {
        let Some(open) = self.open.as_ref() else {
            return InvitePeopleView::default();
        };
        InvitePeopleView {
            target: Some(open.target.clone()),
            search: match open.search {
                Search::Idle => PeopleSearchStatus::Idle,
                Search::Searching => PeopleSearchStatus::Searching,
                Search::Ready => PeopleSearchStatus::Ready,
                Search::Failed(_) => PeopleSearchStatus::Failed,
            },
            results: open
                .results
                .iter()
                .map(|person| result_row(open, person))
                .collect(),
            picked: open.picked.iter().map(picked_row).collect(),
            limited: open.limited,
            sending: open.batch.is_some(),
            error: match open.search {
                Search::Failed(PeopleSearchFailure::RateLimited) => {
                    UserMessage::new(UserMessageKind::RateLimited)
                }
                Search::Failed(PeopleSearchFailure::Failed) => {
                    UserMessage::new(UserMessageKind::PeopleSearchFailed)
                }
                Search::Idle | Search::Searching | Search::Ready => UserMessage::default(),
            },
            avatars_ready: open.avatars_ready,
        }
    }
}

fn result_row(open: &OpenDialog, person: &Arc<Person>) -> InviteeRow {
    let state = if open.picked(&person.user_id).is_some() {
        InviteeState::Picked
    } else {
        match person.membership {
            RoomMembership::Joined => InviteeState::Joined,
            RoomMembership::Invited => InviteeState::Invited,
            RoomMembership::Banned => InviteeState::Banned,
            RoomMembership::Knocking
            | RoomMembership::Left
            | RoomMembership::Outside
            | RoomMembership::Unknown => InviteeState::Available,
        }
    };
    InviteeRow {
        person: Arc::clone(person),
        state,
        failure: UserMessage::default(),
    }
}

fn picked_row(picked: &Picked) -> InviteeRow {
    let (state, failure) = match picked.stage {
        PickStage::Waiting => (InviteeState::Picked, UserMessage::default()),
        PickStage::Sending => (InviteeState::Sending, UserMessage::default()),
        PickStage::Failed(failure) => (
            InviteeState::Failed,
            failure_message(failure, &picked.person.shown()),
        ),
    };
    InviteeRow {
        person: Arc::clone(&picked.person),
        state,
        failure,
    }
}

fn failure_message(failure: InviteFailure, name: &str) -> UserMessage {
    match failure {
        InviteFailure::Blocked => UserMessage::about(UserMessageKind::InviteBlocked, &name),
        InviteFailure::Refused => UserMessage::about(UserMessageKind::InviteRefused, &name),
        InviteFailure::RateLimited => UserMessage::new(UserMessageKind::RateLimited),
        InviteFailure::Failed => UserMessage::about(UserMessageKind::InviteFailed, &name),
    }
}

fn report_after_close(
    output: &dyn AppOutputPort,
    sent: &Batch,
    outcomes: &[(UserId, InviteResult)],
) {
    let failed: Vec<String> = outcomes
        .iter()
        .filter(|(_, outcome)| outcome.is_err())
        .filter_map(|(user_id, _)| {
            sent.people
                .iter()
                .find(|person| person.user_id == *user_id)
                .map(|person| person.shown())
        })
        .collect();
    if failed.is_empty() {
        return;
    }
    tracing::warn!(room_id = %sent.room_id, failed = failed.len(), "some invites failed after the dialog closed");
    show_toast(
        output,
        Toast::Error(UserMessage::about(
            UserMessageKind::InviteFailed,
            &failed.join(", "),
        )),
    );
}
