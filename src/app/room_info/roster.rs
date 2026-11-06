use std::sync::Arc;

use super::{OpenSheet, RoomInfo, RosterOutcome};
use crate::app::avatars::spawn_avatar_batches;
use crate::app::event::{AppEvent, RoomInfoEvent};
use crate::commands::view::{RosterRow, RosterStatus};
use crate::domain::room::RoomId;
use crate::domain::room_info::{MemberQuery, RoomAbout, RosterMember, RosterSection};
use crate::ports::matrix::RoomInfoPort;

pub(super) const MEMBER_PAGE: usize = 50;

pub(super) type Members = Arc<[Arc<RosterMember>]>;

pub(super) enum Roster {
    Loading,
    Failed,
    Ready(Members),
}

impl Roster {
    pub(super) fn status(&self) -> RosterStatus {
        match self {
            Self::Loading => RosterStatus::Loading,
            Self::Failed => RosterStatus::Failed,
            Self::Ready(_) => RosterStatus::Ready,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Fetch {
    Idle,
    InFlight,
    InFlightThenAgain,
}

impl RoomInfo {
    pub(in crate::app) fn members_changed(
        &mut self,
        port: Arc<dyn RoomInfoPort>,
        room_id: &RoomId,
    ) {
        if self.shows(room_id) {
            tracing::debug!(%room_id, "the room's members changed, refetching them");
            self.refetch_roster(port);
        }
    }

    pub(in crate::app) fn page(&mut self, port: Arc<dyn RoomInfoPort>) {
        let Some(open) = self.open.as_mut().filter(|open| open.has_more) else {
            return;
        };
        open.shown = open.shown.saturating_add(MEMBER_PAGE);
        self.reveal(port);
    }

    pub(in crate::app) fn filter(&mut self, port: Arc<dyn RoomInfoPort>, query: String) {
        let Some(open) = self.open.as_mut().filter(|open| open.query != query) else {
            return;
        };
        open.query = query;
        open.shown = MEMBER_PAGE;
        self.reveal(port);
    }

    pub(in crate::app) fn retry(&mut self, port: Arc<dyn RoomInfoPort>) {
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

    pub(in crate::app) fn roster_fetched(
        &mut self,
        port: Arc<dyn RoomInfoPort>,
        generation: u64,
        outcome: RosterOutcome,
    ) {
        let Some(open) = self
            .current(generation)
            .filter(|open| open.fetch != Fetch::Idle)
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

    pub(in crate::app) fn about_fetched(&mut self, generation: u64, about: Option<RoomAbout>) {
        let Some(open) = self.current(generation) else {
            return;
        };
        open.about = about;
        self.publish();
    }

    pub(in crate::app) fn avatars_ready(&mut self, generation: u64, ready: usize) {
        let Some(open) = self.current(generation) else {
            return;
        };
        open.avatars_ready = open.avatars_ready.saturating_add(ready);
        self.publish();
    }

    pub(super) fn refetch_roster(&mut self, port: Arc<dyn RoomInfoPort>) {
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

    pub(super) fn refetch_about(&mut self, port: Arc<dyn RoomInfoPort>) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let (room_id, generation) = (open.card.id.clone(), open.generation);
        self.spawn_about(port, room_id, generation);
    }

    pub(super) fn spawn_roster(
        &mut self,
        port: Arc<dyn RoomInfoPort>,
        room_id: RoomId,
        generation: u64,
    ) {
        self.tasks.spawn_reporting(&self.events, async move {
            let outcome = match port.roster(&room_id).await {
                Ok(members) => RosterOutcome::Loaded(members.into_iter().map(Arc::new).collect()),
                Err(e) => {
                    tracing::warn!(%room_id, "failed to load the member list: {e}");
                    RosterOutcome::Failed
                }
            };
            AppEvent::RoomInfo(RoomInfoEvent::RosterFetched {
                generation,
                outcome,
            })
        });
    }

    pub(super) fn spawn_about(
        &mut self,
        port: Arc<dyn RoomInfoPort>,
        room_id: RoomId,
        generation: u64,
    ) {
        self.tasks.spawn_reporting(&self.events, async move {
            let about = port
                .about(&room_id)
                .await
                .inspect_err(|e| tracing::warn!(%room_id, "failed to read the room's details: {e}"))
                .ok();
            AppEvent::RoomInfo(RoomInfoEvent::AboutFetched { generation, about })
        });
    }

    pub(super) fn spawn_avatars(
        &mut self,
        port: Arc<dyn RoomInfoPort>,
        generation: u64,
        mxcs: Vec<String>,
    ) {
        spawn_avatar_batches(&mut self.tasks, &self.events, port, mxcs, move |ready| {
            AppEvent::RoomInfo(RoomInfoEvent::AvatarsReady { generation, ready })
        });
    }
}

pub(super) fn derive_rows(
    roster: &Roster,
    query: &MemberQuery,
    shown: usize,
) -> (Vec<RosterRow>, bool) {
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

pub(super) fn unrequested_avatars(open: &mut OpenSheet) -> Vec<String> {
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
