use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::mpsc;

use super::selection::Selection;
use super::task_group::TaskGroup;
use crate::commands::UiCommand;
use crate::domain::models::{ConnectionStatus, Room, Space, SyncEvent};
use crate::ports::matrix::MatrixPort;
use crate::ports::output::AppOutputPort;
use crate::ports::storage::StoragePort;

pub(super) struct RoomMeta {
    pub(super) name: String,
    pub(super) member_count: u64,
}

#[derive(Default)]
pub(super) struct ReconcileOutcome {
    pub(super) space_dropped: bool,
    pub(super) subspace_dropped: bool,
}

pub(super) struct RoomDirectory {
    output: Arc<dyn AppOutputPort>,
    all_rooms: Vec<Room>,
    spaces: Vec<Space>,
    space_order: Vec<String>,
    connected: bool,
}

impl RoomDirectory {
    pub(super) fn new(output: Arc<dyn AppOutputPort>) -> Self {
        Self {
            output,
            all_rooms: Vec::new(),
            spaces: Vec::new(),
            space_order: Vec::new(),
            connected: false,
        }
    }

    pub(super) async fn connect(&mut self, storage: &dyn StoragePort) {
        self.connected = true;
        self.space_order = match storage.load_space_order().await {
            Ok(order) => order,
            Err(e) => {
                tracing::warn!("failed to load space order: {e}");
                Vec::new()
            }
        };
    }

    pub(super) fn store_rooms(&mut self, rooms: Vec<Room>) -> bool {
        if !self.connected {
            return false;
        }
        self.all_rooms = rooms;
        true
    }

    pub(super) fn store_spaces(&mut self, spaces: Vec<Space>) -> bool {
        if !self.connected {
            return false;
        }
        self.spaces = spaces;
        true
    }

    pub(super) fn move_space(&mut self, from: usize, to: usize) -> Option<Vec<String>> {
        let mut order: Vec<String> = order_spaces(&root_spaces(&self.spaces), &self.space_order)
            .into_iter()
            .map(|space| space.id)
            .collect();
        if from >= order.len() || to >= order.len() || from == to {
            return None;
        }

        let id = order.remove(from);
        order.insert(to, id);
        self.space_order = order;

        self.emit_spaces();
        Some(self.space_order.clone())
    }

    pub(super) fn reset(&mut self) {
        self.connected = false;
        self.all_rooms.clear();
        self.spaces.clear();
        self.space_order.clear();
    }

    pub(super) fn spawn_sync_pipeline(
        group: &mut TaskGroup,
        matrix: Arc<dyn MatrixPort>,
        output: Arc<dyn AppOutputPort>,
        cmd_tx: mpsc::UnboundedSender<UiCommand>,
    ) {
        let token = group.token();
        let on_sync: Box<dyn Fn(SyncEvent) + Send + Sync> = Box::new(move |event| match event {
            SyncEvent::Connected => {
                output.connection_status(ConnectionStatus::Connected);
            }
            SyncEvent::Rooms(rooms) => {
                cmd_tx.send(UiCommand::RoomsUpdated(rooms)).ok();
            }
            SyncEvent::Spaces(spaces) => {
                cmd_tx.send(UiCommand::SpacesUpdated(spaces)).ok();
            }
            SyncEvent::ConnectionError(msg) => {
                output.connection_status(ConnectionStatus::Error(msg));
            }
            SyncEvent::SessionExpired => {
                cmd_tx.send(UiCommand::SessionExpired).ok();
            }
        });

        group.spawn(async move {
            tokio::select! {
                result = matrix.start_sync(on_sync) => {
                    if let Err(e) = result {
                        tracing::error!("sync loop ended with error: {e}");
                    }
                }
                () = token.cancelled() => {
                    tracing::debug!("sync task cancelled");
                }
            }
        });
    }

    pub(super) fn reconcile(&self, sel: &mut Selection) -> ReconcileOutcome {
        let space_gone = sel
            .space
            .as_ref()
            .is_some_and(|id| find_space(&self.spaces, id).is_none());
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
                .and_then(|parent| find_space(&self.spaces, parent))
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
        let room = self.all_rooms.iter().find(|room| &room.id == id)?;
        Some(RoomMeta {
            name: room.display_name.clone(),
            member_count: if room.is_direct { 0 } else { room.member_count },
        })
    }

    pub(super) fn emit_directory(&self, sel: &Selection) {
        self.emit_spaces();
        self.emit_subspaces(sel);
        self.emit_rooms(sel);
    }

    pub(super) fn emit_rooms(&self, sel: &Selection) {
        let selected = sel.active_filter().map(AsRef::as_ref);
        self.output
            .rooms(filter_rooms(&self.all_rooms, &self.spaces, selected));
    }

    pub(super) fn emit_spaces(&self) {
        let ordered = order_spaces(&root_spaces(&self.spaces), &self.space_order);
        self.output.spaces(self.with_counts(&ordered));
    }

    pub(super) fn emit_subspaces(&self, sel: &Selection) {
        let subspaces: Vec<Space> = sel
            .space
            .as_deref()
            .and_then(|id| find_space(&self.spaces, id))
            .map(|space| {
                space
                    .child_space_ids
                    .iter()
                    .filter_map(|child| find_space(&self.spaces, child))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        self.output.subspaces(self.with_counts(&subspaces));
    }

    fn with_counts(&self, spaces: &[Space]) -> Vec<Space> {
        spaces
            .iter()
            .map(|space| {
                let rooms = descendant_rooms(&self.spaces, &space.id);
                let (unread, mentions) = self
                    .all_rooms
                    .iter()
                    .filter(|room| rooms.contains(room.id.as_ref()))
                    .fold((0_u64, 0_u64), |(unread, mentions), room| {
                        (unread + room.unread_count, mentions + room.mention_count)
                    });
                Space {
                    unread,
                    mentions,
                    ..space.clone()
                }
            })
            .collect()
    }
}

fn find_space<'a>(spaces: &'a [Space], id: &str) -> Option<&'a Space> {
    spaces.iter().find(|space| space.id == id)
}

fn root_spaces(spaces: &[Space]) -> Vec<Space> {
    let nested: HashSet<&str> = spaces
        .iter()
        .flat_map(|space| space.child_space_ids.iter().map(String::as_str))
        .collect();
    spaces
        .iter()
        .filter(|space| !nested.contains(space.id.as_str()))
        .cloned()
        .collect()
}

fn descendant_rooms<'a>(spaces: &'a [Space], root: &'a str) -> HashSet<&'a str> {
    let mut rooms = HashSet::new();
    let mut visited = HashSet::new();
    let mut pending = vec![root];

    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        let Some(space) = find_space(spaces, id) else {
            continue;
        };
        rooms.extend(space.child_room_ids.iter().map(String::as_str));
        pending.extend(space.child_space_ids.iter().map(String::as_str));
    }
    rooms
}

fn filter_rooms(all_rooms: &[Room], spaces: &[Space], selected: Option<&str>) -> Vec<Room> {
    let Some(space_id) = selected else {
        return all_rooms.to_vec();
    };
    let children = descendant_rooms(spaces, space_id);
    all_rooms
        .iter()
        .filter(|room| children.contains(room.id.as_ref()))
        .cloned()
        .collect()
}

fn order_spaces(spaces: &[Space], order: &[String]) -> Vec<Space> {
    let position: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let mut ordered = spaces.to_vec();
    ordered.sort_by_key(|space| {
        position
            .get(space.id.as_str())
            .copied()
            .unwrap_or(usize::MAX)
    });
    ordered
}
