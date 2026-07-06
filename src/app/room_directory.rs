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
    space_children: HashMap<String, HashSet<String>>,
    space_order: Vec<String>,
    selected_space: Option<RoomId>,
    connected: bool,
}

impl RoomDirectory {
    pub(super) fn new(output: Arc<dyn AppOutputPort>) -> Self {
        Self {
            output,
            all_rooms: Vec::new(),
            spaces: Vec::new(),
            space_children: HashMap::new(),
            space_order: Vec::new(),
            selected_space: None,
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
    }

    pub(super) fn update_spaces(&mut self, spaces: Vec<Space>) {
        if !self.connected {
            return;
        }

        self.space_children = spaces
            .iter()
            .map(|space| {
                let children: HashSet<String> = space.child_room_ids.iter().cloned().collect();
                (space.id.clone(), children)
            })
            .collect();
        self.spaces = spaces;

        let selection_gone = self
            .selected_space
            .as_ref()
            .is_some_and(|space| !self.space_children.contains_key(space.as_ref()));
        if selection_gone {
            self.selected_space = None;
        }

        self.emit_spaces();
        self.emit_rooms();
    }

    pub(super) fn select_space(&mut self, space: Option<RoomId>) {
        self.selected_space = space.filter(|id| !id.is_empty());
        self.emit_rooms();
    }

    pub(super) async fn move_space(&mut self, from: usize, to: usize, storage: &dyn StoragePort) {
        let mut order: Vec<String> = order_spaces(&self.spaces, &self.space_order)
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
        self.space_children.clear();
        self.space_order.clear();
        self.selected_space = None;
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

    fn emit_rooms(&self) {
        self.output.rooms(filter_rooms(
            &self.all_rooms,
            &self.space_children,
            self.selected_space.as_deref(),
        ));
    }

    fn emit_spaces(&self) {
        let ordered = order_spaces(&self.spaces, &self.space_order);
        self.output.spaces(aggregate_space_counts(
            &ordered,
            &self.space_children,
            &self.all_rooms,
        ));
    }
}

fn filter_rooms(
    all_rooms: &[Room],
    space_children: &HashMap<String, HashSet<String>>,
    selected: Option<&str>,
) -> Vec<Room> {
    match selected {
        None => all_rooms.to_vec(),
        Some(space_id) => match space_children.get(space_id) {
            Some(children) => all_rooms
                .iter()
                .filter(|room| children.contains(room.id.as_ref()))
                .cloned()
                .collect(),
            None => Vec::new(),
        },
    }
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

fn aggregate_space_counts(
    spaces: &[Space],
    space_children: &HashMap<String, HashSet<String>>,
    all_rooms: &[Room],
) -> Vec<Space> {
    spaces
        .iter()
        .map(|space| {
            let (unread, mentions) = match space_children.get(&space.id) {
                Some(children) => all_rooms
                    .iter()
                    .filter(|room| children.contains(room.id.as_ref()))
                    .fold((0_u64, 0_u64), |(unread, mentions), room| {
                        (unread + room.unread_count, mentions + room.mention_count)
                    }),
                None => (0, 0),
            };
            Space {
                unread,
                mentions,
                ..space.clone()
            }
        })
        .collect()
}
