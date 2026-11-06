use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{Mutex, MutexGuard};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::RoomDirectory;
use crate::app::event::AppEvent;
use crate::app::input::EventSender;
use crate::app::space_order;
use crate::app::task_group::TaskGroup;
use crate::domain::room::RoomId;
use crate::ports::matrix::SpaceOrderPort;

const ORDER_WRITE_ATTEMPTS: u32 = 3;

const ORDER_WRITE_BACKOFF: Duration = Duration::from_millis(400);

pub(super) struct PendingOrder {
    op: u64,
    order: String,
}

#[derive(Default)]
pub(super) struct OrderWrites {
    pub(super) latest_op: AtomicU64,
    in_flight: Mutex<()>,
}

pub(in crate::app) struct SpaceOrderWrite {
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

impl RoomDirectory {
    pub(in crate::app) fn move_space(&mut self, from: usize, to: usize) -> Option<SpaceOrderWrite> {
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

    pub(in crate::app) fn rollback_space_orders(&mut self, op: u64, spaces: &[String]) -> bool {
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

    pub(super) fn reconcile_orders(&mut self) {
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

    pub(in crate::app) fn spawn_order_write(
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

    pub(super) fn ordered_root_indices(&self) -> Vec<usize> {
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
