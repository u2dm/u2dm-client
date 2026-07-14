use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::mpsc;

use super::task_group::TaskGroup;
use crate::commands::UiCommand;
use crate::domain::models::{ConnectionStatus, Room, RoomId, Space, SyncEvent};
use crate::ports::matrix::MatrixPort;
use crate::ports::output::AppOutputPort;
use crate::ports::storage::StoragePort;

pub(super) struct RoomDirectory {
    output: Arc<dyn AppOutputPort>,
    all_rooms: Vec<Room>,
    spaces: Vec<Space>,
    space_order: Vec<String>,
    selected_space: Option<RoomId>,
    selected_subspace: Option<RoomId>,
    connected: bool,
}

impl RoomDirectory {
    pub(super) fn new(output: Arc<dyn AppOutputPort>) -> Self {
        Self {
            output,
            all_rooms: Vec::new(),
            spaces: Vec::new(),
            space_order: Vec::new(),
            selected_space: None,
            selected_subspace: None,
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

    pub(super) fn update_rooms(&mut self, rooms: Vec<Room>) {
        if !self.connected {
            return;
        }

        self.all_rooms = rooms;
        self.emit_rooms();
        self.emit_spaces();
        self.emit_subspaces();
    }

    pub(super) fn update_spaces(&mut self, spaces: Vec<Space>) {
        if !self.connected {
            return;
        }

        self.spaces = spaces;
        self.drop_stale_selection();

        self.emit_spaces();
        self.emit_subspaces();
        self.emit_rooms();
    }

    pub(super) fn select_space(&mut self, space: Option<RoomId>) {
        self.selected_space = space.filter(|id| !id.is_empty());
        self.selected_subspace = None;
        self.emit_subspaces();
        self.emit_rooms();
    }

    pub(super) fn select_subspace(&mut self, subspace: Option<RoomId>) {
        self.selected_subspace = subspace.filter(|id| !id.is_empty());
        self.emit_rooms();
    }

    pub(super) async fn move_space(&mut self, from: usize, to: usize, storage: &dyn StoragePort) {
        let mut order: Vec<String> = order_spaces(&root_spaces(&self.spaces), &self.space_order)
            .into_iter()
            .map(|space| space.id)
            .collect();
        if from >= order.len() || to >= order.len() || from == to {
            return;
        }

        let id = order.remove(from);
        order.insert(to, id);
        self.space_order = order;

        if let Err(e) = storage.save_space_order(&self.space_order).await {
            tracing::warn!("failed to persist space order: {e}");
        }

        self.emit_spaces();
    }

    pub(super) fn reset(&mut self) {
        self.connected = false;
        self.all_rooms.clear();
        self.spaces.clear();
        self.space_order.clear();
        self.selected_space = None;
        self.selected_subspace = None;
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

    fn drop_stale_selection(&mut self) {
        let space_gone = self
            .selected_space
            .as_ref()
            .is_some_and(|id| find_space(&self.spaces, id).is_none());
        if space_gone {
            self.selected_space = None;
            self.selected_subspace = None;
            return;
        }

        let subspace_gone = self.selected_subspace.as_ref().is_some_and(|id| {
            !self
                .selected_space
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
            self.selected_subspace = None;
        }
    }

    fn emit_rooms(&self) {
        let selected = self
            .selected_subspace
            .as_deref()
            .or(self.selected_space.as_deref());
        self.output
            .rooms(filter_rooms(&self.all_rooms, &self.spaces, selected));
    }

    fn emit_spaces(&self) {
        let ordered = order_spaces(&root_spaces(&self.spaces), &self.space_order);
        self.output.spaces(self.with_counts(&ordered));
    }

    fn emit_subspaces(&self) {
        let subspaces: Vec<Space> = self
            .selected_space
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
