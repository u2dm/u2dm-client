use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::event::AppEvent;
use super::input::EventSender;
use super::show_toast;
use super::space_index::AVATAR_BATCH;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{RoomCard, RoomInfoView, RosterRow, RosterStatus, Toast};
use crate::domain::room::{NotifyMode, Room, RoomId};
use crate::domain::room_info::{MemberQuery, RoomAbout, RosterMember, RosterSection};
use crate::ports::matrix::RoomInfoPort;
use crate::ports::output::AppOutputPort;

const MEMBER_PAGE: usize = 50;

type Members = Arc<[Arc<RosterMember>]>;

pub(super) enum RosterOutcome {
    Loaded(Members),
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ActionOutcome {
    Done,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NotifyChange {
    Requested { request: u64, target: NotifyMode },
    AwaitingEcho { target: NotifyMode },
}

impl NotifyChange {
    fn target(self) -> NotifyMode {
        match self {
            Self::Requested { target, .. } | Self::AwaitingEcho { target } => target,
        }
    }
}

enum Roster {
    Loading,
    Failed,
    Ready(Members),
}

impl Roster {
    fn status(&self) -> RosterStatus {
        match self {
            Self::Loading => RosterStatus::Loading,
            Self::Failed => RosterStatus::Failed,
            Self::Ready(_) => RosterStatus::Ready,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fetch {
    Idle,
    InFlight,
    InFlightThenAgain,
}

struct OpenSheet {
    card: RoomCard,
    generation: u64,
    roster: Roster,
    fetch: Fetch,
    about: Option<RoomAbout>,
    query: String,
    shown: usize,
    rows: Arc<[RosterRow]>,
    has_more: bool,
    requested: HashSet<String>,
    avatars_ready: usize,
    error: UserMessage,
}

impl OpenSheet {
    fn adopt(&mut self, outcome: RosterOutcome) {
        match outcome {
            RosterOutcome::Loaded(members) => {
                tracing::debug!(
                    room = %self.card.id,
                    members = members.len(),
                    "loaded the member list"
                );
                self.roster = Roster::Ready(members);
            }
            RosterOutcome::Failed if matches!(self.roster, Roster::Ready(_)) => {
                tracing::debug!(room = %self.card.id, "keeping the member list a refresh failed to replace");
            }
            RosterOutcome::Failed => self.roster = Roster::Failed,
        }
    }
}

pub(super) struct RoomInfo {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    tasks: TaskGroup,
    actions: TaskGroup,
    generation: u64,
    requests: u64,
    pages_landed: i32,
    open: Option<OpenSheet>,
    notify: HashMap<RoomId, NotifyChange>,
    leaving: HashSet<RoomId>,
}

impl RoomInfo {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            tasks: TaskGroup::new("room-info"),
            actions: TaskGroup::new("room-actions"),
            generation: 0,
            requests: 0,
            pages_landed: 0,
            open: None,
            notify: HashMap::new(),
            leaving: HashSet::new(),
        }
    }

    pub(super) fn open(&mut self, port: Arc<dyn RoomInfoPort>, room: Option<&Room>) {
        let Some(room) = room else {
            tracing::debug!("the room is not listed, not opening its room info");
            return;
        };
        if self
            .open
            .as_ref()
            .is_some_and(|open| open.card.id == room.id)
        {
            return;
        }
        self.begin(port, card_of(room));
    }

    pub(super) fn close(&mut self) {
        if self.open.take().is_none() {
            return;
        }
        self.tasks.cancel_and_detach();
        self.publish();
    }

    pub(super) fn follow(&mut self, port: Arc<dyn RoomInfoPort>, rooms: &[Arc<Room>]) {
        let echoed = self.settle_echoes(rooms);
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Some(room) = rooms.iter().find(|room| room.id == open.card.id) else {
            self.close();
            return;
        };
        let card = card_of(room);
        let members_moved = card.member_count != open.card.member_count;
        let about_moved = card.alias != open.card.alias || card.topic != open.card.topic;
        let changed = card != open.card;
        open.card = card;
        if members_moved {
            self.refetch_roster(Arc::clone(&port));
        }
        if about_moved {
            self.refetch_about(port);
        }
        if changed || echoed {
            self.publish();
        }
    }

    pub(super) fn page(&mut self, port: Arc<dyn RoomInfoPort>) {
        let Some(open) = self.open.as_mut().filter(|open| open.has_more) else {
            return;
        };
        open.shown = open.shown.saturating_add(MEMBER_PAGE);
        self.reveal(port);
    }

    pub(super) fn filter(&mut self, port: Arc<dyn RoomInfoPort>, query: String) {
        let Some(open) = self.open.as_mut().filter(|open| open.query != query) else {
            return;
        };
        open.query = query;
        open.shown = MEMBER_PAGE;
        self.reveal(port);
    }

    pub(super) fn retry(&mut self, port: Arc<dyn RoomInfoPort>) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|open| matches!(open.roster, Roster::Failed) && open.fetch == Fetch::Idle)
        else {
            return;
        };
        open.roster = Roster::Loading;
        open.fetch = Fetch::InFlight;
        let (room_id, generation) = (open.card.id.clone(), open.generation);
        self.publish();
        self.spawn_roster(port, room_id, generation);
    }

    pub(super) fn roster_fetched(
        &mut self,
        port: Arc<dyn RoomInfoPort>,
        generation: u64,
        outcome: RosterOutcome,
    ) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|open| open.generation == generation && open.fetch != Fetch::Idle)
        else {
            tracing::debug!(generation, "dropping a superseded member list");
            return;
        };
        let again = open.fetch == Fetch::InFlightThenAgain;
        open.fetch = Fetch::Idle;
        open.adopt(outcome);
        if again {
            self.refetch_roster(Arc::clone(&port));
        }
        self.reveal(port);
    }

    pub(super) fn about_fetched(&mut self, generation: u64, about: Option<RoomAbout>) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|open| open.generation == generation)
        else {
            return;
        };
        open.about = about;
        self.publish();
    }

    pub(super) fn avatars_ready(&mut self, generation: u64, ready: usize) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|open| open.generation == generation)
        else {
            return;
        };
        open.avatars_ready = open.avatars_ready.saturating_add(ready);
        self.publish();
    }

    pub(super) fn set_notify(&mut self, port: Arc<dyn RoomInfoPort>, mode: NotifyMode) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let room_id = open.card.id.clone();
        let change = self.notify.get(&room_id).copied();
        if self.leaving.contains(&room_id)
            || matches!(change, Some(NotifyChange::Requested { .. }))
            || change.map_or(open.card.notify, NotifyChange::target) == mode
        {
            tracing::debug!(%room_id, ?mode, "ignoring a notification change that has nothing to do");
            return;
        }
        self.requests = self.requests.wrapping_add(1);
        let request = self.requests;
        self.notify.insert(
            room_id.clone(),
            NotifyChange::Requested {
                request,
                target: mode,
            },
        );
        open.error = UserMessage::default();
        let name = open.card.name.clone();
        self.publish();

        let events = self.events.clone();
        let cancel = self.actions.token();
        self.actions.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                written = port.set_notify(&room_id, mode) => match written {
                    Ok(()) => ActionOutcome::Done,
                    Err(e) => {
                        tracing::warn!(%room_id, "failed to change the room's notifications: {e}");
                        ActionOutcome::Failed
                    }
                },
            };
            drop(events.send(AppEvent::RoomNotifySettled {
                request,
                room_id,
                name,
                outcome,
            }));
        });
    }

    pub(super) fn notify_settled(
        &mut self,
        request: u64,
        room_id: RoomId,
        name: &str,
        outcome: ActionOutcome,
        listed: Option<NotifyMode>,
    ) {
        let Some(NotifyChange::Requested {
            request: pending,
            target,
        }) = self.notify.get(&room_id).copied()
        else {
            tracing::debug!(%room_id, "dropping a notification change nobody is waiting for");
            return;
        };
        if pending != request {
            return;
        }
        match outcome {
            ActionOutcome::Done if listed.is_none_or(|mode| mode == target) => {
                self.notify.remove(&room_id);
            }
            ActionOutcome::Done => {
                tracing::debug!(%room_id, "changed, waiting for sync to deliver the push rules");
                self.notify
                    .insert(room_id, NotifyChange::AwaitingEcho { target });
            }
            ActionOutcome::Failed => {
                self.notify.remove(&room_id);
                self.report(
                    &room_id,
                    UserMessage::about(UserMessageKind::NotifyChangeFailed, &name),
                );
            }
        }
        self.publish();
    }

    pub(super) fn leave(&mut self, port: Arc<dyn RoomInfoPort>, room_id: RoomId) {
        let Some(open) = self.open.as_mut().filter(|open| open.card.id == room_id) else {
            tracing::debug!(%room_id, "ignoring a leave for a room the room info does not show");
            return;
        };
        if !self.leaving.insert(room_id.clone()) {
            return;
        }
        open.error = UserMessage::default();
        let name = open.card.name.clone();
        self.publish();

        let events = self.events.clone();
        let cancel = self.actions.token();
        self.actions.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                left = port.leave(&room_id) => match left {
                    Ok(()) => ActionOutcome::Done,
                    Err(e) => {
                        tracing::warn!(%room_id, "failed to leave the room: {e}");
                        ActionOutcome::Failed
                    }
                },
            };
            drop(events.send(AppEvent::RoomLeaveSettled {
                room_id,
                name,
                outcome,
            }));
        });
    }

    pub(super) fn leave_settled(&mut self, room_id: &RoomId, name: &str, outcome: ActionOutcome) {
        if !self.leaving.remove(room_id) {
            tracing::debug!(%room_id, "dropping a leave nobody is waiting for");
            return;
        }
        match outcome {
            ActionOutcome::Done => {
                if self
                    .open
                    .as_ref()
                    .is_some_and(|open| open.card.id == *room_id)
                {
                    self.close();
                }
            }
            ActionOutcome::Failed => {
                self.report(
                    room_id,
                    UserMessage::about(UserMessageKind::LeaveRoomFailed, &name),
                );
                self.publish();
            }
        }
    }

    pub(super) async fn restart(&mut self) {
        tokio::join!(self.tasks.restart(), self.actions.restart());
        self.open = None;
        self.notify.clear();
        self.leaving.clear();
    }

    pub(super) async fn shutdown(&mut self) {
        tokio::join!(self.tasks.shutdown(), self.actions.shutdown());
    }

    fn begin(&mut self, port: Arc<dyn RoomInfoPort>, card: RoomCard) {
        self.tasks.cancel_and_detach();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let room_id = card.id.clone();
        tracing::debug!(room = %room_id, "opening the room info");
        self.open = Some(OpenSheet {
            card,
            generation,
            roster: Roster::Loading,
            fetch: Fetch::InFlight,
            about: None,
            query: String::new(),
            shown: MEMBER_PAGE,
            rows: Arc::from(Vec::new()),
            has_more: false,
            requested: HashSet::new(),
            avatars_ready: 0,
            error: UserMessage::default(),
        });
        self.publish();
        self.spawn_about(Arc::clone(&port), room_id.clone(), generation);
        self.spawn_roster(port, room_id, generation);
    }

    fn settle_echoes(&mut self, rooms: &[Arc<Room>]) -> bool {
        let before = self.notify.len();
        self.notify.retain(|room_id, change| match change {
            NotifyChange::Requested { .. } => true,
            NotifyChange::AwaitingEcho { target } => rooms
                .iter()
                .find(|room| room.id == *room_id)
                .is_some_and(|room| room.notify != *target),
        });
        self.notify.len() != before
    }

    fn refetch_roster(&mut self, port: Arc<dyn RoomInfoPort>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if open.fetch != Fetch::Idle {
            open.fetch = Fetch::InFlightThenAgain;
            return;
        }
        open.fetch = Fetch::InFlight;
        let (room_id, generation) = (open.card.id.clone(), open.generation);
        self.spawn_roster(port, room_id, generation);
    }

    fn refetch_about(&mut self, port: Arc<dyn RoomInfoPort>) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let (room_id, generation) = (open.card.id.clone(), open.generation);
        self.spawn_about(port, room_id, generation);
    }

    fn reveal(&mut self, port: Arc<dyn RoomInfoPort>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let (rows, has_more) =
            derive_rows(&open.roster, &MemberQuery::new(&open.query), open.shown);
        if *open.rows != *rows || open.has_more != has_more {
            open.rows = rows.into();
            open.has_more = has_more;
            self.pages_landed = self.pages_landed.wrapping_add(1);
        }
        let wanted = unrequested_avatars(open);
        let generation = open.generation;
        self.publish();
        self.spawn_avatars(port, generation, wanted);
    }

    fn report(&mut self, room_id: &RoomId, message: UserMessage) {
        match self.open.as_mut().filter(|open| open.card.id == *room_id) {
            Some(open) => open.error = message,
            None => show_toast(self.output.as_ref(), Toast::Error(message)),
        }
    }

    fn spawn_roster(&mut self, port: Arc<dyn RoomInfoPort>, room_id: RoomId, generation: u64) {
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                roster = port.roster(&room_id) => match roster {
                    Ok(members) => RosterOutcome::Loaded(members.into_iter().map(Arc::new).collect()),
                    Err(e) => {
                        tracing::warn!(%room_id, "failed to load the member list: {e}");
                        RosterOutcome::Failed
                    }
                },
            };
            drop(events.send(AppEvent::RoomRosterFetched {
                generation,
                outcome,
            }));
        });
    }

    fn spawn_about(&mut self, port: Arc<dyn RoomInfoPort>, room_id: RoomId, generation: u64) {
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let about = tokio::select! {
                () = cancel.cancelled() => return,
                about = port.about(&room_id) => about
                    .inspect_err(|e| tracing::warn!(%room_id, "failed to read the room's details: {e}"))
                    .ok(),
            };
            drop(events.send(AppEvent::RoomAboutFetched { generation, about }));
        });
    }

    fn spawn_avatars(&mut self, port: Arc<dyn RoomInfoPort>, generation: u64, mxcs: Vec<String>) {
        if mxcs.is_empty() {
            return;
        }
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            for batch in mxcs.chunks(AVATAR_BATCH) {
                let ready = tokio::select! {
                    () = cancel.cancelled() => return,
                    ready = port.fetch_avatars(batch) => ready,
                };
                if ready > 0 {
                    drop(events.send(AppEvent::RoomInfoAvatarsReady { generation, ready }));
                }
            }
        });
    }

    fn publish(&self) {
        let view = self
            .open
            .as_ref()
            .map_or_else(RoomInfoView::default, |open| {
                let change = self.notify.get(&open.card.id).copied();
                RoomInfoView {
                    card: Some(open.card.clone()),
                    about: open.about.clone(),
                    roster: open.roster.status(),
                    rows: Arc::clone(&open.rows),
                    has_more: open.has_more,
                    pages_landed: self.pages_landed,
                    avatars_ready: open.avatars_ready,
                    notify: change.map_or(open.card.notify, NotifyChange::target),
                    notify_busy: matches!(change, Some(NotifyChange::Requested { .. })),
                    leaving: self.leaving.contains(&open.card.id),
                    error: open.error.clone(),
                }
            });
        self.output
            .publish(Box::new(move |state| state.room_info = view));
    }
}

fn card_of(room: &Room) -> RoomCard {
    RoomCard {
        id: room.id.clone(),
        name: room.display_name.clone(),
        avatar_mxc: room.avatar_mxc.clone(),
        member_count: room.member_count,
        is_direct: room.is_direct,
        topic: room.topic.clone(),
        alias: room.canonical_alias.clone(),
        notify: room.notify,
    }
}

fn derive_rows(roster: &Roster, query: &MemberQuery, shown: usize) -> (Vec<RosterRow>, bool) {
    let Roster::Ready(members) = roster else {
        return (Vec::new(), false);
    };
    let mut matching = members.iter().filter(|member| member.matches(query));
    let mut rows = Vec::new();
    let mut invited_heading_placed = false;
    for member in matching.by_ref().take(shown) {
        if member.section == RosterSection::Invited && !invited_heading_placed {
            rows.push(RosterRow::InvitedHeading);
            invited_heading_placed = true;
        }
        rows.push(RosterRow::Member(Arc::clone(member)));
    }
    let has_more = matching.next().is_some();
    (rows, has_more)
}

fn unrequested_avatars(open: &mut OpenSheet) -> Vec<String> {
    let OpenSheet {
        rows, requested, ..
    } = open;
    rows.iter()
        .filter_map(|row| match row {
            RosterRow::Member(member) => member.avatar_mxc.clone(),
            RosterRow::InvitedHeading => None,
        })
        .filter(|mxc| requested.insert(mxc.clone()))
        .collect()
}
