use std::collections::{HashMap, HashSet};
use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{Mutex, MutexGuard, mpsc};
use tokio::time::{Instant, sleep};
use tokio_util::sync::CancellationToken;

use super::selection::Selection;
use super::space_order;
use super::task_group::TaskGroup;
use crate::commands::sync::DirectoryUpdate;
use crate::commands::ui::UiCommand;
use crate::domain::models::{
    ConnectionStatus, Room, RoomId, RoomList, Space, SyncEvent, SyncOutcome,
};
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

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct SpaceCounts {
    unread: u64,
    mentions: u64,
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
    assignments: Vec<(String, String)>,
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
    counts: Vec<SpaceCounts>,
    spaces_dirty: bool,
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
            counts: Vec::new(),
            spaces_dirty: false,
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
        self.spaces_dirty |= self.recompute_counts();
        true
    }

    pub(super) fn store_spaces(&mut self, spaces: Arc<[Space]>) -> bool {
        if !self.connected {
            return false;
        }
        self.spaces = spaces;
        self.reconcile_orders();
        self.graph = SpaceGraph::build(&self.spaces);
        self.recompute_counts();
        self.spaces_dirty = true;
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
        for (id, order) in &assignments {
            self.orders.insert(id.clone(), order.clone());
            self.pending_orders.insert(
                id.clone(),
                PendingOrder {
                    op,
                    order: order.clone(),
                },
            );
        }

        self.emit_spaces();
        Some(SpaceOrderWrite {
            op,
            writes: Arc::clone(&self.order_writes),
            assignments,
        })
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
        self.counts.clear();
        self.spaces_dirty = false;
        self.orders.clear();
        self.pending_orders.clear();
        self.order_writes.latest_op.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn spawn_order_write(
        group: &mut TaskGroup,
        port: Arc<dyn SpaceOrderPort>,
        write: SpaceOrderWrite,
        cmd_tx: mpsc::UnboundedSender<UiCommand>,
    ) {
        let token = group.token();
        group.spawn(async move {
            let SpaceOrderWrite {
                op,
                writes,
                assignments,
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
            for (space_id, order) in assignments {
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
                drop(cmd_tx.send(UiCommand::SpaceOrderWriteFailed {
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
        cmd_tx: mpsc::UnboundedSender<UiCommand>,
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

        group.spawn(supervise_sync(sync, output, cmd_tx, on_sync, token));
    }

    pub(super) fn reconcile(&self, sel: &mut Selection) -> ReconcileOutcome {
        let space_gone = sel
            .space
            .as_ref()
            .is_some_and(|id| self.space(id).is_none());
        if space_gone {
            sel.space = None;
            sel.subspace = None;
            return ReconcileOutcome {
                space_dropped: true,
                subspace_dropped: true,
            };
        }

        let subspace_gone = sel.subspace.as_ref().is_some_and(|id| {
            !sel.space
                .as_ref()
                .and_then(|parent| self.space(parent))
                .is_some_and(|parent| {
                    parent
                        .child_space_ids
                        .iter()
                        .any(|child| child == id.as_ref())
                })
        });
        if subspace_gone {
            sel.subspace = None;
        }

        ReconcileOutcome {
            space_dropped: false,
            subspace_dropped: subspace_gone,
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
        if mem::take(&mut self.spaces_dirty) {
            self.emit_spaces();
            self.emit_subspaces(sel);
        }
        self.emit_rooms(sel);
    }

    pub(super) fn emit_rooms(&self, sel: &Selection) {
        let rooms = match sel.active_filter().map(AsRef::as_ref) {
            None => Arc::clone(&self.all_rooms),
            Some(space_id) => match self.graph.index.get(space_id).copied() {
                Some(space_index) => self
                    .all_rooms
                    .iter()
                    .filter(|room| self.graph.contains_room(space_index, room.id.as_ref()))
                    .map(Arc::clone)
                    .collect::<Vec<Arc<Room>>>()
                    .into(),
                None => Arc::from(Vec::new()),
            },
        };
        self.output
            .publish(Box::new(move |view| view.directory.rooms = rooms));
    }

    pub(super) fn emit_spaces(&self) {
        let spaces: Vec<Space> = self
            .ordered_root_indices()
            .into_iter()
            .filter_map(|i| self.space_with_counts(i))
            .collect();
        let spaces: Arc<[Space]> = spaces.into();
        self.output
            .publish(Box::new(move |view| view.directory.spaces = spaces));
    }

    pub(super) fn emit_subspaces(&self, sel: &Selection) {
        let subspaces: Vec<Space> = sel
            .space
            .as_deref()
            .and_then(|id| self.space(id))
            .map(|space| {
                space
                    .child_space_ids
                    .iter()
                    .filter_map(|child| self.graph.index.get(child).copied())
                    .filter_map(|i| self.space_with_counts(i))
                    .collect()
            })
            .unwrap_or_default();
        let subspaces: Arc<[Space]> = subspaces.into();
        self.output
            .publish(Box::new(move |view| view.directory.subspaces = subspaces));
    }

    fn space_with_counts(&self, space_index: usize) -> Option<Space> {
        let space = self.spaces.get(space_index)?;
        let counts = self.counts.get(space_index).copied().unwrap_or_default();
        Some(Space {
            unread: counts.unread,
            mentions: counts.mentions,
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

    fn recompute_counts(&mut self) -> bool {
        let mut next = vec![SpaceCounts::default(); self.spaces.len()];
        for room in self.all_rooms.iter() {
            for &i in self.graph.ancestors_of(room.id.as_ref()) {
                let Some(slot) = next.get_mut(i) else {
                    continue;
                };
                slot.unread = slot.unread.saturating_add(room.unread_count);
                slot.mentions = slot.mentions.saturating_add(room.mention_count);
            }
        }
        let changed = next != self.counts;
        self.counts = next;
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
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    on_sync: SyncSink,
    token: CancellationToken,
) {
    let mut backoff = BACKOFF_START;
    loop {
        let started = Instant::now();
        match sync.start_sync(Arc::clone(&on_sync), token.clone()).await {
            SyncOutcome::Cancelled => return,
            SyncOutcome::SessionExpired => {
                drop(cmd_tx.send(UiCommand::SessionExpired));
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
