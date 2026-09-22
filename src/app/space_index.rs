use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::event::AppEvent;
use super::input::EventSender;
use super::room_directory::Membership;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{ChildAccess, SpaceIndexRow, SpaceIndexStatus, SpaceIndexView, Toast};
use crate::domain::room::RoomId;
use crate::domain::space_index::{HierarchyPage, JoinRule, SpaceChild};
use crate::ports::matrix::SpaceIndexPort;
use crate::ports::output::AppOutputPort;

const AVATAR_BATCH: usize = 8;

pub(super) struct ListedSpace {
    pub(super) id: RoomId,
    pub(super) name: String,
}

pub(super) enum PageOutcome {
    Loaded(Box<HierarchyPage>),
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum JoinOutcome {
    Joined,
    Failed,
}

pub(super) enum ChildTarget {
    Room,
    Subspace,
}

enum Fetch {
    FirstPage,
    NextPage { from: String },
    Idle { next: Option<String> },
    FirstPageFailed,
    NextPageFailed { from: String },
}

impl Fetch {
    fn status(&self) -> SpaceIndexStatus {
        match self {
            Self::FirstPage => SpaceIndexStatus::Loading,
            Self::NextPage { .. } => SpaceIndexStatus::LoadingMore,
            Self::Idle { next: Some(_) } => SpaceIndexStatus::Partial,
            Self::Idle { next: None } => SpaceIndexStatus::Complete,
            Self::FirstPageFailed => SpaceIndexStatus::Failed,
            Self::NextPageFailed { .. } => SpaceIndexStatus::MoreFailed,
        }
    }

    fn in_flight(&self) -> bool {
        matches!(self, Self::FirstPage | Self::NextPage { .. })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JoinStage {
    Requested,
    AwaitingSync,
}

struct OpenIndex {
    space: ListedSpace,
    generation: u64,
    fetch: Fetch,
    children: Vec<Arc<SpaceChild>>,
    rows: Arc<[SpaceIndexRow]>,
    avatars_ready: usize,
}

struct PageRequest {
    space: RoomId,
    from: Option<String>,
    generation: u64,
}

pub(super) struct SpaceIndex {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    tasks: TaskGroup,
    joins: TaskGroup,
    generation: u64,
    open: Option<OpenIndex>,
    pending: HashMap<RoomId, JoinStage>,
}

impl SpaceIndex {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            tasks: TaskGroup::new("space-index"),
            joins: TaskGroup::new("space-joins"),
            generation: 0,
            open: None,
            pending: HashMap::new(),
        }
    }

    pub(super) fn is_tracking(&self) -> bool {
        self.open.is_some() || !self.pending.is_empty()
    }

    pub(super) fn open(&mut self, port: Arc<dyn SpaceIndexPort>, target: Option<ListedSpace>) {
        let Some(target) = target else {
            tracing::debug!("no space is selected, not opening the space index");
            return;
        };
        if self
            .open
            .as_ref()
            .is_some_and(|open| open.space.id == target.id)
        {
            return;
        }
        self.begin(port, target);
    }

    pub(super) fn follow(
        &mut self,
        port: Arc<dyn SpaceIndexPort>,
        target: Option<ListedSpace>,
        membership: &Membership<'_>,
    ) {
        let Some(open) = self.open.as_mut() else {
            self.membership_changed(membership);
            return;
        };
        match target {
            None => self.close(),
            Some(target) if target.id == open.space.id => {
                let renamed = open.space.name != target.name;
                open.space.name = target.name;
                if self.rederive(membership) || renamed {
                    self.publish();
                }
            }
            Some(target) => self.begin(port, target),
        }
    }

    pub(super) fn close(&mut self) {
        if self.open.take().is_none() {
            return;
        }
        self.tasks.cancel_and_detach();
        self.pending
            .retain(|_, stage| *stage == JoinStage::Requested);
        self.publish();
    }

    pub(super) fn page(&mut self, port: Arc<dyn SpaceIndexPort>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Fetch::Idle { next: Some(from) } = &open.fetch else {
            return;
        };
        let from = from.clone();
        open.fetch = Fetch::NextPage { from: from.clone() };
        let request = PageRequest {
            space: open.space.id.clone(),
            from: Some(from),
            generation: open.generation,
        };
        self.publish();
        self.spawn_page(port, request);
    }

    pub(super) fn retry(&mut self, port: Arc<dyn SpaceIndexPort>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let from = match &open.fetch {
            Fetch::FirstPageFailed => None,
            Fetch::NextPageFailed { from } => Some(from.clone()),
            Fetch::FirstPage | Fetch::NextPage { .. } | Fetch::Idle { .. } => return,
        };
        open.fetch = match &from {
            Some(from) => Fetch::NextPage { from: from.clone() },
            None => Fetch::FirstPage,
        };
        let request = PageRequest {
            space: open.space.id.clone(),
            from,
            generation: open.generation,
        };
        self.publish();
        self.spawn_page(port, request);
    }

    pub(super) fn fetched(
        &mut self,
        port: Arc<dyn SpaceIndexPort>,
        generation: u64,
        outcome: PageOutcome,
        membership: &Membership<'_>,
    ) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|open| open.generation == generation && open.fetch.in_flight())
        else {
            tracing::debug!(generation, "dropping a superseded space index page");
            return;
        };
        match outcome {
            PageOutcome::Loaded(page) => {
                let HierarchyPage { children, next } = *page;
                let mut known: HashSet<RoomId> =
                    open.children.iter().map(|child| child.id.clone()).collect();
                let fresh: Vec<Arc<SpaceChild>> = children
                    .into_iter()
                    .filter(|child| known.insert(child.id.clone()))
                    .map(Arc::new)
                    .collect();
                let avatars: Vec<String> = fresh
                    .iter()
                    .filter_map(|child| child.avatar_mxc.clone())
                    .collect();
                tracing::debug!(
                    space = %open.space.id,
                    rows = fresh.len(),
                    more = next.is_some(),
                    "loaded a space index page"
                );
                open.children.extend(fresh);
                open.fetch = Fetch::Idle { next };
                self.rederive(membership);
                self.publish();
                self.spawn_avatars(port, generation, avatars);
            }
            PageOutcome::Failed => {
                open.fetch = match &open.fetch {
                    Fetch::NextPage { from } => Fetch::NextPageFailed { from: from.clone() },
                    Fetch::FirstPage
                    | Fetch::Idle { .. }
                    | Fetch::FirstPageFailed
                    | Fetch::NextPageFailed { .. } => Fetch::FirstPageFailed,
                };
                self.publish();
            }
        }
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

    pub(super) fn join(
        &mut self,
        port: Arc<dyn SpaceIndexPort>,
        room_id: RoomId,
        membership: &Membership<'_>,
    ) {
        let Some(child) = self.child(&room_id) else {
            tracing::debug!(%room_id, "ignoring a join for a room the space index does not list");
            return;
        };
        if access(&child, membership, false) != ChildAccess::Join
            || self.pending.contains_key(&room_id)
        {
            tracing::debug!(%room_id, "ignoring a join for a room that cannot be joined");
            return;
        }
        self.pending.insert(room_id.clone(), JoinStage::Requested);
        if self.rederive(membership) {
            self.publish();
        }

        let events = self.events.clone();
        let cancel = self.joins.token();
        let name = child.name.clone();
        let via = child.via.clone();
        self.joins.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                joined = port.join(&room_id, &via) => match joined {
                    Ok(()) => JoinOutcome::Joined,
                    Err(e) => {
                        tracing::warn!(%room_id, "failed to join a room from the space index: {e}");
                        JoinOutcome::Failed
                    }
                },
            };
            drop(events.send(AppEvent::SpaceChildJoinSettled {
                room_id,
                name,
                outcome,
            }));
        });
    }

    pub(super) fn join_settled(
        &mut self,
        room_id: RoomId,
        name: &str,
        outcome: JoinOutcome,
        membership: &Membership<'_>,
    ) {
        if self.pending.get(&room_id) != Some(&JoinStage::Requested) {
            tracing::debug!(%room_id, "dropping a join result nobody is waiting for");
            return;
        }
        match outcome {
            JoinOutcome::Failed => {
                self.pending.remove(&room_id);
                show_toast(
                    self.output.as_ref(),
                    Toast::Error(UserMessage::about(UserMessageKind::JoinRoomFailed, &name)),
                );
            }
            JoinOutcome::Joined if membership.has_joined(&room_id) => {
                self.pending.remove(&room_id);
            }
            JoinOutcome::Joined => {
                tracing::debug!(%room_id, "joined, waiting for sync to deliver the room");
                self.pending.insert(room_id, JoinStage::AwaitingSync);
            }
        }
        if self.rederive(membership) {
            self.publish();
        }
    }

    pub(super) fn membership_changed(&mut self, membership: &Membership<'_>) {
        self.pending
            .retain(|id, stage| *stage == JoinStage::Requested || !membership.has_joined(id));
        if self.rederive(membership) {
            self.publish();
        }
    }

    pub(super) fn open_target(
        &self,
        room_id: &RoomId,
        membership: &Membership<'_>,
    ) -> Option<ChildTarget> {
        let child = self.child(room_id)?;
        match access(&child, membership, false) {
            ChildAccess::Open => Some(ChildTarget::Room),
            ChildAccess::OpenSubspace => Some(ChildTarget::Subspace),
            ChildAccess::Joined
            | ChildAccess::Joining
            | ChildAccess::Join
            | ChildAccess::InviteOnly
            | ChildAccess::Knock
            | ChildAccess::MembersOnly
            | ChildAccess::Unavailable => None,
        }
    }

    pub(super) async fn restart(&mut self) {
        tokio::join!(self.tasks.restart(), self.joins.restart());
        self.open = None;
        self.pending.clear();
    }

    pub(super) async fn shutdown(&mut self) {
        tokio::join!(self.tasks.shutdown(), self.joins.shutdown());
    }

    fn begin(&mut self, port: Arc<dyn SpaceIndexPort>, space: ListedSpace) {
        self.tasks.cancel_and_detach();
        self.generation = self.generation.wrapping_add(1);
        let request = PageRequest {
            space: space.id.clone(),
            from: None,
            generation: self.generation,
        };
        tracing::debug!(space = %space.id, "opening the space index");
        self.open = Some(OpenIndex {
            space,
            generation: self.generation,
            fetch: Fetch::FirstPage,
            children: Vec::new(),
            rows: Arc::from(Vec::new()),
            avatars_ready: 0,
        });
        self.publish();
        self.spawn_page(port, request);
    }

    fn child(&self, room_id: &RoomId) -> Option<Arc<SpaceChild>> {
        self.open
            .as_ref()?
            .children
            .iter()
            .find(|child| child.id == *room_id)
            .map(Arc::clone)
    }

    fn rederive(&mut self, membership: &Membership<'_>) -> bool {
        let pending = &self.pending;
        let Some(open) = self.open.as_mut() else {
            return false;
        };
        let rows: Vec<SpaceIndexRow> = open
            .children
            .iter()
            .map(|child| SpaceIndexRow {
                access: access(child, membership, pending.contains_key(&child.id)),
                child: Arc::clone(child),
            })
            .collect();
        if *open.rows == *rows {
            return false;
        }
        open.rows = rows.into();
        true
    }

    fn spawn_page(&mut self, port: Arc<dyn SpaceIndexPort>, request: PageRequest) {
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let PageRequest {
                space,
                from,
                generation,
            } = request;
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                page = port.hierarchy_page(&space, from.as_deref()) => match page {
                    Ok(page) => PageOutcome::Loaded(Box::new(page)),
                    Err(e) => {
                        tracing::warn!(%space, "failed to load the space index: {e}");
                        PageOutcome::Failed
                    }
                },
            };
            drop(events.send(AppEvent::SpaceIndexPaged {
                generation,
                outcome,
            }));
        });
    }

    fn spawn_avatars(&mut self, port: Arc<dyn SpaceIndexPort>, generation: u64, mxcs: Vec<String>) {
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
                    drop(events.send(AppEvent::SpaceIndexAvatarsReady { generation, ready }));
                }
            }
        });
    }

    fn publish(&self) {
        let view = self
            .open
            .as_ref()
            .map_or_else(SpaceIndexView::default, |open| SpaceIndexView {
                status: open.fetch.status(),
                space_name: open.space.name.clone(),
                rows: Arc::clone(&open.rows),
                avatars_ready: open.avatars_ready,
            });
        self.output
            .publish(Box::new(move |state| state.space_index = view));
    }
}

fn access(child: &SpaceChild, membership: &Membership<'_>, joining: bool) -> ChildAccess {
    if membership.has_space(&child.id) {
        return if membership.in_rail(&child.id) {
            ChildAccess::OpenSubspace
        } else {
            ChildAccess::Joined
        };
    }
    if membership.has_room(&child.id) {
        return ChildAccess::Open;
    }
    if joining {
        return ChildAccess::Joining;
    }
    match &child.join_rule {
        JoinRule::Public => ChildAccess::Join,
        JoinRule::Restricted { allowed } | JoinRule::KnockRestricted { allowed }
            if membership.admits(allowed) =>
        {
            ChildAccess::Join
        }
        JoinRule::Restricted { .. } => ChildAccess::MembersOnly,
        JoinRule::KnockRestricted { .. } | JoinRule::Knock => ChildAccess::Knock,
        JoinRule::Invite => ChildAccess::InviteOnly,
        JoinRule::Private | JoinRule::Unsupported => ChildAccess::Unavailable,
    }
}
