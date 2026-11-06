mod listing;
mod rail;
mod space_graph;
mod space_orders;
mod sync_supervisor;

use std::collections::{HashMap, HashSet};
use std::mem;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use self::space_graph::{SpaceGraph, mark_reachable};
use self::space_orders::{OrderWrites, PendingOrder};
use super::selection::{RoomFilter, Selection};
use crate::commands::view::{ChildAccess, InviteTarget, InvitesSection};
use crate::domain::departure::{Removal, RemovalList, SpaceDeparture};
use crate::domain::event_size::EventIds;
use crate::domain::invite::{Invite, InviteList};
use crate::domain::message::MessagePermissions;
use crate::domain::poll::PollPermissions;
use crate::domain::room::{
    Room, RoomId, RoomList, Space, SpacePowers, UnreadFlags, without_replaced,
};
use crate::domain::room_search::TitleQuery;
use crate::domain::space_index::JoinRule;
use crate::ports::output::AppOutputPort;

#[derive(Default, PartialEq, Eq)]
pub(super) struct RoomMeta {
    pub(super) name: String,
    pub(super) avatar_mxc: Option<String>,
    pub(super) member_count: u64,
    pub(super) encrypted: bool,
    pub(super) event_ids: EventIds,
    pub(super) readable_by_anyone: bool,
    pub(super) polls: PollPermissions,
    pub(super) messages: MessagePermissions,
}

#[derive(Default)]
pub(super) struct ReconcileOutcome {
    pub(super) space_dropped: bool,
    pub(super) subspace_dropped: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum InvitesFold {
    #[default]
    Folded,
    Unfolded,
}

pub(super) struct Membership<'a> {
    rooms: HashSet<&'a str>,
    spaces: &'a HashMap<String, usize>,
    space_list: &'a [Space],
    rail: &'a [String],
}

impl Membership<'_> {
    pub(super) fn has_room(&self, id: &str) -> bool {
        self.rooms.contains(id)
    }

    pub(super) fn has_space(&self, id: &str) -> bool {
        self.spaces.contains_key(id)
    }

    pub(super) fn in_rail(&self, id: &str) -> bool {
        self.rail.iter().any(|child| child == id)
    }

    pub(super) fn has_joined(&self, id: &str) -> bool {
        self.has_room(id) || self.has_space(id)
    }

    pub(super) fn lists_child(&self, parent: &str, child: &str) -> bool {
        self.spaces
            .get(parent)
            .and_then(|&i| self.space_list.get(i))
            .is_some_and(|space| {
                space
                    .child_room_ids
                    .iter()
                    .chain(&space.child_space_ids)
                    .any(|id| id == child)
            })
    }

    pub(super) fn admits(&self, allowed: &[RoomId]) -> bool {
        allowed.is_empty() || allowed.iter().any(|id| self.has_joined(id))
    }

    pub(super) fn join_access(&self, rule: &JoinRule) -> ChildAccess {
        match rule {
            JoinRule::Public => ChildAccess::Join,
            JoinRule::Restricted { allowed } | JoinRule::KnockRestricted { allowed }
                if self.admits(allowed) =>
            {
                ChildAccess::Join
            }
            JoinRule::Restricted { .. } => ChildAccess::MembersOnly,
            JoinRule::KnockRestricted { .. } | JoinRule::Knock => ChildAccess::Knock,
            JoinRule::Invite => ChildAccess::InviteOnly,
            JoinRule::Private | JoinRule::Unsupported => ChildAccess::Unavailable,
        }
    }
}

pub(super) struct RoomDirectory {
    output: Arc<dyn AppOutputPort>,
    all_rooms: RoomList,
    listable: RoomList,
    received_invites: InviteList,
    invites: InviteList,
    received_removals: RemovalList,
    removals: RemovalList,
    forgotten: HashSet<RoomId>,
    spaces: Arc<[Space]>,
    graph: SpaceGraph,
    flags: Vec<UnreadFlags>,
    direct_flags: UnreadFlags,
    attention: usize,
    rail_dirty: bool,
    orders: HashMap<String, String>,
    pending_orders: HashMap<String, PendingOrder>,
    order_writes: Arc<OrderWrites>,
    connected: bool,
    rooms_landed: bool,
    query: TitleQuery,
    invites_fold: InvitesFold,
}

impl RoomDirectory {
    pub(super) fn new(output: Arc<dyn AppOutputPort>) -> Self {
        Self {
            output,
            all_rooms: Arc::from(Vec::new()),
            listable: Arc::from(Vec::new()),
            received_invites: Arc::from(Vec::new()),
            invites: Arc::from(Vec::new()),
            received_removals: Arc::from(Vec::new()),
            removals: Arc::from(Vec::new()),
            forgotten: HashSet::new(),
            spaces: Arc::from(Vec::new()),
            graph: SpaceGraph::default(),
            flags: Vec::new(),
            direct_flags: UnreadFlags::default(),
            attention: 0,
            rail_dirty: false,
            orders: HashMap::new(),
            pending_orders: HashMap::new(),
            order_writes: Arc::default(),
            connected: false,
            rooms_landed: false,
            query: TitleQuery::default(),
            invites_fold: InvitesFold::default(),
        }
    }

    pub(super) fn connect(&mut self) {
        self.connected = true;
    }

    pub(super) fn store_rooms(&mut self, rooms: RoomList) -> bool {
        if !self.connected {
            return false;
        }
        self.listable = without_replaced(&rooms);
        self.all_rooms = rooms;
        self.rooms_landed = true;
        self.settle_removals();
        let pending_changed = self.settle_invites();
        self.rail_dirty |= self.recompute_flags() || pending_changed;
        true
    }

    pub(super) fn store_invites(&mut self, invites: InviteList) -> bool {
        if !self.connected {
            return false;
        }
        self.received_invites = invites;
        let pending_changed = self.settle_invites();
        self.settle_removals();
        self.rail_dirty |= self.recompute_flags() || pending_changed;
        true
    }

    pub(super) fn store_removals(&mut self, removals: RemovalList) -> bool {
        if !self.connected {
            return false;
        }
        self.forgotten
            .retain(|id| removals.iter().any(|removal| removal.id == *id));
        self.received_removals = removals;
        self.settle_removals();
        true
    }

    pub(super) fn forgot(&mut self, room_id: &RoomId) {
        if self
            .received_removals
            .iter()
            .any(|removal| removal.id == *room_id)
        {
            self.forgotten.insert(room_id.clone());
        }
        self.settle_removals();
    }

    fn settle_removals(&mut self) {
        let listed: HashSet<&str> = self
            .all_rooms
            .iter()
            .map(|room| room.id.as_ref())
            .chain(self.spaces.iter().map(|space| space.id.as_str()))
            .chain(
                self.received_invites
                    .iter()
                    .map(|invite| invite.id.as_ref()),
            )
            .chain(self.forgotten.iter().map(AsRef::as_ref))
            .collect();
        self.removals = self
            .received_removals
            .iter()
            .filter(|removal| !listed.contains(removal.id.as_ref()))
            .map(Arc::clone)
            .collect::<Vec<Arc<Removal>>>()
            .into();
    }

    fn settle_invites(&mut self) -> bool {
        let was_empty = self.invites.is_empty();
        let joined: HashSet<&str> = self
            .all_rooms
            .iter()
            .map(|room| room.id.as_ref())
            .chain(self.spaces.iter().map(|space| space.id.as_str()))
            .collect();
        self.invites = if self
            .received_invites
            .iter()
            .any(|invite| joined.contains(invite.id.as_ref()))
        {
            self.received_invites
                .iter()
                .filter(|invite| !joined.contains(invite.id.as_ref()))
                .map(Arc::clone)
                .collect::<Vec<Arc<Invite>>>()
                .into()
        } else {
            Arc::clone(&self.received_invites)
        };
        was_empty != self.invites.is_empty()
    }

    pub(super) fn store_spaces(&mut self, spaces: Arc<[Space]>) -> bool {
        if !self.connected {
            return false;
        }
        self.spaces = spaces;
        self.reconcile_orders();
        self.graph = SpaceGraph::build(&self.spaces);
        self.settle_invites();
        self.settle_removals();
        self.recompute_flags();
        self.rail_dirty = true;
        true
    }

    pub(super) fn reset(&mut self) {
        self.connected = false;
        self.rooms_landed = false;
        self.all_rooms = Arc::from(Vec::new());
        self.listable = Arc::from(Vec::new());
        self.received_invites = Arc::from(Vec::new());
        self.invites = Arc::from(Vec::new());
        self.received_removals = Arc::from(Vec::new());
        self.removals = Arc::from(Vec::new());
        self.forgotten.clear();
        self.spaces = Arc::from(Vec::new());
        self.graph = SpaceGraph::default();
        self.flags.clear();
        self.direct_flags = UnreadFlags::default();
        self.attention = 0;
        self.rail_dirty = false;
        self.orders.clear();
        self.pending_orders.clear();
        self.order_writes.latest_op.fetch_add(1, Ordering::Relaxed);
        self.query = TitleQuery::default();
        self.invites_fold = InvitesFold::default();
    }

    pub(super) fn search(&mut self, query: TitleQuery) -> bool {
        if self.query == query {
            return false;
        }
        self.query = query;
        true
    }

    pub(super) fn fold_invites(&mut self, fold: InvitesFold) -> bool {
        if self.invites_fold == fold {
            return false;
        }
        self.invites_fold = fold;
        true
    }

    fn invites_section(&self, sel: &Selection) -> InvitesSection {
        let searching = !self.query.is_empty() && !matches!(sel.filter, RoomFilter::Space { .. });
        match (searching, self.invites_fold) {
            (true, _) => InvitesSection::Matches,
            (false, InvitesFold::Folded) => InvitesSection::Folded,
            (false, InvitesFold::Unfolded) => InvitesSection::Unfolded,
        }
    }

    pub(super) fn reconcile(&self, sel: &mut Selection) -> ReconcileOutcome {
        let RoomFilter::Space { space, subspace } = &mut sel.filter else {
            return ReconcileOutcome::default();
        };
        let Some(parent) = self.space(space) else {
            sel.filter = RoomFilter::All;
            return ReconcileOutcome {
                space_dropped: true,
                subspace_dropped: true,
            };
        };

        let subspace_gone = subspace.as_ref().is_some_and(|id| {
            !parent
                .child_space_ids
                .iter()
                .any(|child| child == id.as_ref())
        });
        if subspace_gone {
            *subspace = None;
        }

        ReconcileOutcome {
            space_dropped: false,
            subspace_dropped: subspace_gone,
        }
    }

    pub(super) fn space_name(&self, id: &str) -> Option<&str> {
        self.space(id).map(|space| space.name.as_str())
    }

    pub(super) fn membership(&self, sel: &Selection) -> Membership<'_> {
        Membership {
            rooms: self.all_rooms.iter().map(|room| room.id.as_ref()).collect(),
            spaces: &self.graph.index,
            space_list: &self.spaces,
            rail: sel
                .space()
                .and_then(|id| self.space(id))
                .map_or(&[] as &[String], |space| space.child_space_ids.as_slice()),
        }
    }

    pub(super) fn selected_room_meta(&self, sel: &Selection) -> Option<RoomMeta> {
        let id = sel.room.as_ref()?;
        let room = self.room(id)?;
        Some(RoomMeta {
            name: room.display_name.clone(),
            avatar_mxc: room.avatar_mxc.clone(),
            member_count: if room.is_direct { 0 } else { room.member_count },
            encrypted: room.is_encrypted,
            event_ids: room.event_ids,
            readable_by_anyone: room.readable_by_anyone(),
            polls: room.poll_permissions,
            messages: room.message_permissions,
        })
    }

    pub(super) fn emit_directory(&mut self, sel: &Selection) {
        if mem::take(&mut self.rail_dirty) {
            self.emit_spaces();
            self.emit_subspaces(sel);
            self.emit_listed_space(sel);
            self.emit_rail_flags();
        }
        self.emit_rooms(sel);
    }

    pub(super) fn emit_rooms(&self, sel: &Selection) {
        let rooms = self.listed(&self.listable, sel);
        let invites = self.listed(&self.invites, sel);
        let invites_section = self.invites_section(sel);
        let removals = self.listed(&self.removals, sel);
        let space_matches = self.space_matches(sel);
        self.output.publish(Box::new(move |view| {
            view.directory.rooms = rooms;
            view.directory.invites = invites;
            view.directory.invites_section = invites_section;
            view.directory.removals = removals;
            view.directory.space_matches = space_matches;
        }));
    }

    pub(super) fn rooms(&self) -> &[Arc<Room>] {
        &self.all_rooms
    }

    pub(super) fn invites(&self) -> &[Arc<Invite>] {
        &self.invites
    }

    pub(super) fn invite(&self, id: &str) -> Option<&Arc<Invite>> {
        self.invites.iter().find(|invite| invite.id.as_ref() == id)
    }

    pub(super) fn spaces(&self) -> &[Space] {
        &self.spaces
    }

    pub(super) fn removals(&self) -> &[Arc<Removal>] {
        &self.removals
    }

    pub(super) fn removal(&self, id: &str) -> Option<&Arc<Removal>> {
        self.removals
            .iter()
            .find(|removal| removal.id.as_ref() == id)
    }

    pub(super) fn rooms_landed(&self) -> bool {
        self.rooms_landed
    }

    pub(super) fn room_by_alias(&self, alias: &str) -> Option<&Room> {
        self.all_rooms
            .iter()
            .find(|room| room.canonical_alias.as_deref() == Some(alias))
            .map(|room| &**room)
    }

    pub(super) fn room(&self, id: &str) -> Option<&Room> {
        self.all_rooms
            .iter()
            .find(|room| room.id.as_ref() == id)
            .map(|room| &**room)
    }

    pub(super) fn space(&self, id: &str) -> Option<&Space> {
        self.graph.index.get(id).and_then(|&i| self.spaces.get(i))
    }

    pub(super) fn space_departure(&self, space_id: &str) -> SpaceDeparture {
        let Some(&root) = self.graph.index.get(space_id) else {
            return SpaceDeparture::default();
        };
        let mut subtree = HashSet::new();
        mark_reachable(&self.spaces, &self.graph.index, [root], &mut subtree);
        let kept = self.kept_without(root, &subtree);
        let mut departing: Vec<usize> = subtree
            .iter()
            .copied()
            .filter(|i| !kept.contains(i))
            .collect();
        departing.sort_unstable();
        let departing: Vec<&Space> = departing
            .into_iter()
            .filter_map(|i| self.spaces.get(i))
            .collect();
        let kept_rooms: HashSet<&str> = kept
            .iter()
            .filter_map(|&i| self.spaces.get(i))
            .flat_map(|space| space.child_room_ids.iter().map(String::as_str))
            .collect();
        let departing_rooms: HashSet<&str> = departing
            .iter()
            .flat_map(|space| space.child_room_ids.iter().map(String::as_str))
            .filter(|id| !kept_rooms.contains(id))
            .collect();
        SpaceDeparture {
            rooms: self
                .listable
                .iter()
                .filter(|room| departing_rooms.contains(room.id.as_ref()))
                .map(|room| room.id.clone())
                .collect(),
            spaces: departing
                .iter()
                .filter(|space| space.id != space_id)
                .map(|space| RoomId::new(space.id.clone()))
                .collect(),
        }
    }

    fn kept_without(&self, root: usize, subtree: &HashSet<usize>) -> HashSet<usize> {
        let outside: Vec<usize> = (0..self.spaces.len())
            .filter(|i| !subtree.contains(i))
            .collect();
        let mut kept = HashSet::from([root]);
        mark_reachable(&self.spaces, &self.graph.index, outside, &mut kept);
        kept.remove(&root);
        kept
    }

    pub(super) fn space_powers(&self, id: &str) -> SpacePowers {
        self.space(id).map(|space| space.powers).unwrap_or_default()
    }

    pub(super) fn invite_target(&self, id: &RoomId) -> Option<InviteTarget> {
        if let Some(room) = self.room(id).filter(|room| room.may_invite) {
            return Some(InviteTarget {
                room_id: id.clone(),
                name: room.display_name.clone(),
                space: false,
            });
        }
        self.space(id)
            .filter(|space| space.powers.invite)
            .map(|space| InviteTarget {
                room_id: id.clone(),
                name: space.name.clone(),
                space: true,
            })
    }
}
