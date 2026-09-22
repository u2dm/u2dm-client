use std::collections::{HashMap, HashSet};
use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{Mutex, MutexGuard, mpsc};
use tokio::time::{Instant, sleep};
use tokio_util::sync::CancellationToken;

use super::event::{AppEvent, SessionEvent};
use super::input::EventSender;
use super::selection::{RoomFilter, Selection};
use super::space_order;
use super::task_group::TaskGroup;
use crate::commands::sync::DirectoryUpdate;
use crate::domain::room::{Room, RoomId, RoomList, Space, UnreadFlags};
use crate::domain::sync::{ConnectionStatus, SessionLoss, SyncEvent, SyncOutcome};
use crate::ports::matrix::{SpaceOrderPort, SyncPort, SyncSink};
use crate::ports::output::AppOutputPort;

const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_mins(1);
const BACKOFF_RESET_AFTER: Duration = Duration::from_mins(1);
const ORDER_WRITE_ATTEMPTS: u32 = 3;
const ORDER_WRITE_BACKOFF: Duration = Duration::from_millis(400);

pub(super) struct RoomMeta {
    pub(super) name: String,
    pub(super) member_count: u64,
}

#[derive(Default)]
pub(super) struct ReconcileOutcome {
    pub(super) space_dropped: bool,
    pub(super) subspace_dropped: bool,
}

pub(super) struct Membership<'a> {
    rooms: HashSet<&'a str>,
    spaces: &'a HashMap<String, usize>,
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

    pub(super) fn admits(&self, allowed: &[RoomId]) -> bool {
        allowed.is_empty() || allowed.iter().any(|id| self.has_joined(id))
    }
}

#[derive(Default)]
struct SpaceGraph {
    index: HashMap<String, usize>,
    root_indices: Vec<usize>,
    room_ancestors: HashMap<String, Vec<usize>>,
}

impl SpaceGraph {
    fn build(spaces: &[Space]) -> Self {
        let index: HashMap<String, usize> = spaces
            .iter()
            .enumerate()
            .map(|(i, space)| (space.id.clone(), i))
            .collect();

        let nested: HashSet<&str> = spaces
            .iter()
            .flat_map(|space| space.child_space_ids.iter().map(String::as_str))
            .collect();
        let mut root_indices: Vec<usize> = spaces
            .iter()
            .enumerate()
            .filter(|(_, space)| !nested.contains(space.id.as_str()))
            .map(|(i, _)| i)
            .collect();

        let mut reachable: HashSet<usize> = HashSet::new();
        mark_reachable(spaces, &index, root_indices.iter().copied(), &mut reachable);
        for i in 0..spaces.len() {
            if reachable.contains(&i) {
                continue;
            }
            root_indices.push(i);
            mark_reachable(spaces, &index, [i], &mut reachable);
        }

        let mut room_ancestors: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, space) in spaces.iter().enumerate() {
            for room_id in descendant_rooms(spaces, &index, space) {
                room_ancestors
                    .entry(room_id.to_owned())
                    .or_default()
                    .push(i);
            }
        }

        Self {
            index,
            root_indices,
            room_ancestors,
        }
    }

    fn ancestors_of(&self, room_id: &str) -> &[usize] {
        self.room_ancestors
            .get(room_id)
            .map_or(&[] as &[usize], Vec::as_slice)
    }

    fn contains_room(&self, space_index: usize, room_id: &str) -> bool {
        self.ancestors_of(room_id).contains(&space_index)
    }
}

fn mark_reachable(
    spaces: &[Space],
    index: &HashMap<String, usize>,
    seeds: impl IntoIterator<Item = usize>,
    reachable: &mut HashSet<usize>,
) {
    let mut stack: Vec<usize> = seeds.into_iter().collect();
    while let Some(i) = stack.pop() {
        if !reachable.insert(i) {
            continue;
        }
        if let Some(space) = spaces.get(i) {
            stack.extend(
                space
                    .child_space_ids
                    .iter()
                    .filter_map(|id| index.get(id).copied()),
            );
        }
    }
}

fn descendant_rooms<'a>(
    spaces: &'a [Space],
    index: &HashMap<String, usize>,
    root: &'a Space,
) -> HashSet<&'a str> {
    let mut rooms = HashSet::new();
    let mut visited = HashSet::new();
    let mut pending = vec![root];

    while let Some(space) = pending.pop() {
        if !visited.insert(space.id.as_str()) {
            continue;
        }
        rooms.extend(space.child_room_ids.iter().map(String::as_str));
        pending.extend(
            space
                .child_space_ids
                .iter()
                .filter_map(|id| index.get(id))
                .filter_map(|&i| spaces.get(i)),
        );
    }
    rooms
}

struct PendingOrder {
    op: u64,
    order: String,
}

#[derive(Default)]
struct OrderWrites {
    latest_op: AtomicU64,
    in_flight: Mutex<()>,
}

pub(super) struct SpaceOrderWrite {
    op: u64,
    writes: Arc<OrderWrites>,
    outstanding: Vec<(String, String)>,
}

struct OrderWriteGuard {
    op: u64,
    writes: Arc<OrderWrites>,
    token: CancellationToken,
}

impl OrderWriteGuard {
    fn is_current(&self) -> bool {
        self.writes.latest_op.load(Ordering::Relaxed) == self.op && !self.token.is_cancelled()
    }
}

enum OrderWriteStep {
    Written,
    Superseded,
    Failed(String),
}

pub(super) struct RoomDirectory {
    output: Arc<dyn AppOutputPort>,
    all_rooms: RoomList,
    spaces: Arc<[Space]>,
    graph: SpaceGraph,
    flags: Vec<UnreadFlags>,
    direct_flags: UnreadFlags,
    rail_dirty: bool,
    orders: HashMap<String, String>,
    pending_orders: HashMap<String, PendingOrder>,
    order_writes: Arc<OrderWrites>,
    connected: bool,
}

impl RoomDirectory {
    pub(super) fn new(output: Arc<dyn AppOutputPort>) -> Self {
        Self {
            output,
            all_rooms: Arc::from(Vec::new()),
            spaces: Arc::from(Vec::new()),
            graph: SpaceGraph::default(),
            flags: Vec::new(),
            direct_flags: UnreadFlags::default(),
            rail_dirty: false,
            orders: HashMap::new(),
            pending_orders: HashMap::new(),
            order_writes: Arc::default(),
            connected: false,
        }
    }

    pub(super) fn connect(&mut self) {
        self.connected = true;
    }

    pub(super) fn store_rooms(&mut self, rooms: RoomList) -> bool {
        if !self.connected {
            return false;
        }
        self.all_rooms = rooms;
        self.rail_dirty |= self.recompute_flags();
        true
    }

    pub(super) fn store_spaces(&mut self, spaces: Arc<[Space]>) -> bool {
        if !self.connected {
            return false;
        }
        self.spaces = spaces;
        self.reconcile_orders();
        self.graph = SpaceGraph::build(&self.spaces);
        self.recompute_flags();
        self.rail_dirty = true;
        true
    }

    pub(super) fn move_space(&mut self, from: usize, to: usize) -> Option<SpaceOrderWrite> {
        let mut target = self.ordered_root_ids();
        if from >= target.len() || to >= target.len() || from == to {
            return None;
        }

        let id = target.remove(from);
        target.insert(to, id);

        let assignments = self.assign_orders(&target, to);
        if assignments.is_empty() {
            return None;
        }

        let op = self
            .order_writes
            .latest_op
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        for (id, order) in assignments {
            self.orders.insert(id.clone(), order.clone());
            self.pending_orders.insert(id, PendingOrder { op, order });
        }

        let outstanding = self.claim_outstanding_orders(op);
        self.emit_spaces();
        Some(SpaceOrderWrite {
            op,
            writes: Arc::clone(&self.order_writes),
            outstanding,
        })
    }

    fn claim_outstanding_orders(&mut self, op: u64) -> Vec<(String, String)> {
        for pending in self.pending_orders.values_mut() {
            pending.op = op;
        }
        let mut outstanding: Vec<(String, String)> = self
            .pending_orders
            .iter()
            .map(|(id, pending)| (id.clone(), pending.order.clone()))
            .collect();
        outstanding.sort_by(|(a_id, a_order), (b_id, b_order)| {
            a_order.cmp(b_order).then_with(|| a_id.cmp(b_id))
        });
        outstanding
    }

    pub(super) fn rollback_space_orders(&mut self, op: u64, spaces: &[String]) -> bool {
        let mut reverted = false;
        for id in spaces {
            if self
                .pending_orders
                .get(id)
                .is_some_and(|pending| pending.op == op)
            {
                self.pending_orders.remove(id);
                reverted = true;
            }
        }
        if reverted {
            self.reconcile_orders();
            self.emit_spaces();
        }
        reverted
    }

    fn reconcile_orders(&mut self) {
        self.forget_departed_spaces();

        let len = self.spaces.len();
        for i in 0..len {
            let Some((id, server)) = self
                .spaces
                .get(i)
                .map(|space| (space.id.clone(), space.order.clone()))
            else {
                continue;
            };
            let pending = self
                .pending_orders
                .get(&id)
                .map(|pending| pending.order.clone());
            match (pending, server) {
                (Some(local), server) => {
                    if server.as_ref() == Some(&local) {
                        self.pending_orders.remove(&id);
                    }
                    self.orders.insert(id, local);
                }
                (None, Some(server)) => {
                    self.orders.insert(id, server);
                }
                (None, None) => {
                    self.orders.remove(&id);
                }
            }
        }
    }

    fn forget_departed_spaces(&mut self) {
        let live: HashSet<&str> = self.spaces.iter().map(|space| space.id.as_str()).collect();
        self.orders.retain(|id, _| live.contains(id.as_str()));
        self.pending_orders
            .retain(|id, _| live.contains(id.as_str()));
    }

    fn assign_orders(&self, target: &[String], moved: usize) -> Vec<(String, String)> {
        let all_ordered = target.iter().all(|id| self.orders.contains_key(id));
        if all_ordered {
            let left = moved
                .checked_sub(1)
                .and_then(|i| target.get(i))
                .and_then(|id| self.orders.get(id))
                .map(String::as_str);
            let right = target
                .get(moved + 1)
                .and_then(|id| self.orders.get(id))
                .map(String::as_str);
            if let Some(order) = space_order::between(left, right)
                && let Some(id) = target.get(moved)
            {
                return vec![(id.clone(), order)];
            }
        }
        self.rebalance(target)
    }

    fn rebalance(&self, target: &[String]) -> Vec<(String, String)> {
        let orders = space_order::even_orders(target.len());
        let mut changed = Vec::new();
        for (id, order) in target.iter().zip(orders) {
            if self.orders.get(id) != Some(&order) {
                changed.push((id.clone(), order));
            }
        }
        changed
    }

    pub(super) fn reset(&mut self) {
        self.connected = false;
        self.all_rooms = Arc::from(Vec::new());
        self.spaces = Arc::from(Vec::new());
        self.graph = SpaceGraph::default();
        self.flags.clear();
        self.direct_flags = UnreadFlags::default();
        self.rail_dirty = false;
        self.orders.clear();
        self.pending_orders.clear();
        self.order_writes.latest_op.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn spawn_order_write(
        group: &mut TaskGroup,
        port: Arc<dyn SpaceOrderPort>,
        write: SpaceOrderWrite,
        events: EventSender,
    ) {
        let token = group.token();
        group.spawn(async move {
            let SpaceOrderWrite {
                op,
                writes,
                outstanding,
            } = write;
            let guard = OrderWriteGuard {
                op,
                writes: Arc::clone(&writes),
                token,
            };
            let Some(_in_flight) = acquire_write_lane(&writes, &guard).await else {
                return;
            };

            let mut failed = Vec::new();
            let mut error = String::new();
            for (space_id, order) in outstanding {
                match write_space_order(&port, &space_id, &order, &guard).await {
                    OrderWriteStep::Written => {}
                    OrderWriteStep::Superseded => {
                        tracing::debug!(op, "space order write superseded, abandoning");
                        return;
                    }
                    OrderWriteStep::Failed(e) => {
                        tracing::warn!(%space_id, "giving up on space order write: {e}");
                        error = e;
                        failed.push(space_id);
                    }
                }
            }

            if !failed.is_empty() {
                drop(events.send(AppEvent::SpaceOrderWriteFailed {
                    op,
                    spaces: failed,
                    error,
                }));
            }
        });
    }

    pub(super) fn spawn_sync_pipeline(
        group: &mut TaskGroup,
        sync: Arc<dyn SyncPort>,
        output: Arc<dyn AppOutputPort>,
        events: EventSender,
        dir_in_tx: mpsc::UnboundedSender<DirectoryUpdate>,
    ) {
        let token = group.token();
        let sink_output = Arc::clone(&output);
        let on_sync: SyncSink = Arc::new(move |event| match event {
            SyncEvent::Connected => {
                sink_output.publish(Box::new(|view| {
                    view.connection = ConnectionStatus::Connected;
                }));
            }
            SyncEvent::Rooms(rooms) => {
                drop(dir_in_tx.send(DirectoryUpdate::Rooms(rooms)));
            }
            SyncEvent::Spaces(spaces) => {
                drop(dir_in_tx.send(DirectoryUpdate::Spaces(spaces)));
            }
            SyncEvent::ConnectionError(msg) => {
                sink_output.publish(Box::new(move |view| {
                    view.connection = ConnectionStatus::Error(msg);
                }));
            }
        });

        group.spawn(supervise_sync(sync, output, events, on_sync, token));
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
            member_count: if room.is_direct { 0 } else { room.member_count },
        })
    }

    pub(super) fn emit_directory(&mut self, sel: &Selection) {
        if mem::take(&mut self.rail_dirty) {
            self.emit_spaces();
            self.emit_subspaces(sel);
            self.emit_direct_flags();
        }
        self.emit_rooms(sel);
    }

    pub(super) fn emit_rooms(&self, sel: &Selection) {
        let rooms = match &sel.filter {
            RoomFilter::All => Arc::clone(&self.all_rooms),
            RoomFilter::Direct => self.rooms_where(|room| room.is_direct),
            RoomFilter::Space { space, subspace } => {
                let space_id = subspace.as_ref().unwrap_or(space);
                match self.graph.index.get(space_id.as_ref()).copied() {
                    Some(space_index) => self.rooms_where(|room| {
                        self.graph.contains_room(space_index, room.id.as_ref())
                    }),
                    None => Arc::from(Vec::new()),
                }
            }
        };
        self.output
            .publish(Box::new(move |view| view.directory.rooms = rooms));
    }

    fn rooms_where(&self, keep: impl Fn(&Room) -> bool) -> RoomList {
        self.all_rooms
            .iter()
            .filter(|room| keep(room))
            .map(Arc::clone)
            .collect::<Vec<Arc<Room>>>()
            .into()
    }

    pub(super) fn emit_spaces(&self) {
        let spaces: Vec<Space> = self
            .ordered_root_indices()
            .into_iter()
            .filter_map(|i| self.space_with_flags(i))
            .collect();
        let spaces: Arc<[Space]> = spaces.into();
        self.output
            .publish(Box::new(move |view| view.directory.spaces = spaces));
    }

    pub(super) fn emit_subspaces(&self, sel: &Selection) {
        let subspaces: Vec<Space> = sel
            .space()
            .and_then(|id| self.space(id))
            .map(|space| {
                space
                    .child_space_ids
                    .iter()
                    .filter_map(|child| self.graph.index.get(child).copied())
                    .filter_map(|i| self.space_with_flags(i))
                    .collect()
            })
            .unwrap_or_default();
        let subspaces: Arc<[Space]> = subspaces.into();
        self.output
            .publish(Box::new(move |view| view.directory.subspaces = subspaces));
    }

    fn emit_direct_flags(&self) {
        let flags = self.direct_flags;
        self.output
            .publish(Box::new(move |view| view.directory.direct_flags = flags));
    }

    fn space_with_flags(&self, space_index: usize) -> Option<Space> {
        let space = self.spaces.get(space_index)?;
        let flags = self.flags.get(space_index).copied().unwrap_or_default();
        Some(Space {
            alert: flags.alert,
            mention: flags.mention,
            hint: flags.hint,
            ..space.clone()
        })
    }

    fn room(&self, id: &str) -> Option<&Room> {
        self.all_rooms
            .iter()
            .find(|room| room.id.as_ref() == id)
            .map(|room| &**room)
    }

    fn space(&self, id: &str) -> Option<&Space> {
        self.graph.index.get(id).and_then(|&i| self.spaces.get(i))
    }

    fn recompute_flags(&mut self) -> bool {
        let mut next = vec![UnreadFlags::default(); self.spaces.len()];
        let mut direct = UnreadFlags::default();
        for room in self.all_rooms.iter() {
            for &i in self.graph.ancestors_of(room.id.as_ref()) {
                let Some(slot) = next.get_mut(i) else {
                    continue;
                };
                slot.absorb(room);
            }
            if room.is_direct {
                direct.absorb(room);
            }
        }
        let changed = next != self.flags || direct != self.direct_flags;
        self.flags = next;
        self.direct_flags = direct;
        changed
    }

    fn ordered_root_indices(&self) -> Vec<usize> {
        let mut indices = self.graph.root_indices.clone();
        indices.sort_by(|&a, &b| self.order_key(a).cmp(&self.order_key(b)));
        indices
    }

    fn order_key(&self, index: usize) -> (bool, Option<&str>, &str) {
        let id = self
            .spaces
            .get(index)
            .map(|space| space.id.as_str())
            .unwrap_or_default();
        let order = self.orders.get(id).map(String::as_str);
        (order.is_none(), order, id)
    }

    fn ordered_root_ids(&self) -> Vec<String> {
        self.ordered_root_indices()
            .into_iter()
            .filter_map(|i| self.spaces.get(i))
            .map(|space| space.id.clone())
            .collect()
    }
}

async fn acquire_write_lane<'a>(
    writes: &'a OrderWrites,
    guard: &OrderWriteGuard,
) -> Option<MutexGuard<'a, ()>> {
    tokio::select! {
        () = guard.token.cancelled() => None,
        lane = writes.in_flight.lock() => guard.is_current().then_some(lane),
    }
}

async fn write_space_order(
    port: &Arc<dyn SpaceOrderPort>,
    space_id: &str,
    order: &str,
    guard: &OrderWriteGuard,
) -> OrderWriteStep {
    let room_id = RoomId::new(space_id.to_owned());
    let mut backoff = ORDER_WRITE_BACKOFF;
    let mut attempt = 1;
    loop {
        if !guard.is_current() {
            return OrderWriteStep::Superseded;
        }
        let Err(e) = port.set_space_order(&room_id, order).await else {
            return OrderWriteStep::Written;
        };
        if attempt >= ORDER_WRITE_ATTEMPTS {
            return OrderWriteStep::Failed(e.to_string());
        }
        tracing::debug!(%room_id, attempt, "space order write failed, retrying: {e}");
        tokio::select! {
            () = guard.token.cancelled() => return OrderWriteStep::Superseded,
            () = sleep(backoff) => {}
        }
        backoff = backoff.saturating_mul(2);
        attempt = attempt.saturating_add(1);
    }
}

fn publish_connection(output: &Arc<dyn AppOutputPort>, status: ConnectionStatus) {
    output.publish(Box::new(move |view| view.connection = status));
}

async fn supervise_sync(
    sync: Arc<dyn SyncPort>,
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    on_sync: SyncSink,
    token: CancellationToken,
) {
    let mut backoff = BACKOFF_START;
    loop {
        let started = Instant::now();
        match sync.start_sync(Arc::clone(&on_sync), token.clone()).await {
            SyncOutcome::Cancelled => return,
            SyncOutcome::SessionLost(loss) => {
                let event = match loss {
                    SessionLoss::SoftLogout => SessionEvent::Suspended,
                    SessionLoss::Expired => SessionEvent::Expired,
                };
                drop(events.send(AppEvent::Session(event)));
                return;
            }
            SyncOutcome::Fatal(msg) => {
                tracing::error!("sync failed unrecoverably: {msg}");
                publish_connection(&output, ConnectionStatus::Error(msg));
                return;
            }
            SyncOutcome::Recoverable(msg) => {
                if started.elapsed() >= BACKOFF_RESET_AFTER {
                    backoff = BACKOFF_START;
                }
                tracing::warn!("sync ended, retrying in {backoff:?}: {msg}");
                publish_connection(&output, ConnectionStatus::Error(msg));
                tokio::select! {
                    () = token.cancelled() => return,
                    () = sleep(backoff) => {}
                }
                backoff = backoff.saturating_mul(2).min(BACKOFF_MAX);
            }
        }
    }
}
