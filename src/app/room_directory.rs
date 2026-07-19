use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{mpsc, watch};

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
    all_rooms: Arc<[Room]>,
    room_index: HashMap<String, usize>,
    spaces: Arc<[Space]>,
    space_index: HashMap<String, usize>,
    root_space_ids: Vec<String>,
    space_counts: HashMap<String, (u64, u64)>,
    space_order: Vec<String>,
    connected: bool,
}

impl RoomDirectory {
    pub(super) fn new(output: Arc<dyn AppOutputPort>) -> Self {
        Self {
            output,
            all_rooms: Arc::from(Vec::new()),
            room_index: HashMap::new(),
            spaces: Arc::from(Vec::new()),
            space_index: HashMap::new(),
            root_space_ids: Vec::new(),
            space_counts: HashMap::new(),
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

    pub(super) fn store_rooms(&mut self, rooms: Arc<[Room]>) -> bool {
        if !self.connected {
            return false;
        }
        self.all_rooms = rooms;
        self.rebuild_room_index();
        self.rebuild_space_counts();
        true
    }

    pub(super) fn store_spaces(&mut self, spaces: Arc<[Space]>) -> bool {
        if !self.connected {
            return false;
        }
        self.spaces = spaces;
        self.rebuild_space_index();
        self.rebuild_space_counts();
        true
    }

    pub(super) fn move_space(&mut self, from: usize, to: usize) -> Option<Vec<String>> {
        let mut order = self.ordered_root_ids();
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
        self.all_rooms = Arc::from(Vec::new());
        self.room_index.clear();
        self.spaces = Arc::from(Vec::new());
        self.space_index.clear();
        self.root_space_ids.clear();
        self.space_counts.clear();
        self.space_order.clear();
    }

    pub(super) fn spawn_sync_pipeline(
        group: &mut TaskGroup,
        matrix: Arc<dyn MatrixPort>,
        output: Arc<dyn AppOutputPort>,
        cmd_tx: mpsc::UnboundedSender<UiCommand>,
        rooms_in_tx: watch::Sender<Arc<[Room]>>,
        spaces_in_tx: watch::Sender<Arc<[Space]>>,
    ) {
        let token = group.token();
        let on_sync: Box<dyn Fn(SyncEvent) + Send + Sync> = Box::new(move |event| match event {
            SyncEvent::Connected => {
                output.connection_status(ConnectionStatus::Connected);
            }
            SyncEvent::Rooms(rooms) => {
                drop(rooms_in_tx.send(rooms));
            }
            SyncEvent::Spaces(spaces) => {
                drop(spaces_in_tx.send(spaces));
            }
            SyncEvent::ConnectionError(msg) => {
                output.connection_status(ConnectionStatus::Error(msg));
            }
            SyncEvent::SessionExpired => {
                drop(cmd_tx.send(UiCommand::SessionExpired));
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

    pub(super) fn emit_directory(&self, sel: &Selection) {
        self.emit_spaces();
        self.emit_subspaces(sel);
        self.emit_rooms(sel);
    }

    pub(super) fn emit_rooms(&self, sel: &Selection) {
        let rooms = match sel.active_filter().map(AsRef::as_ref) {
            None => Arc::clone(&self.all_rooms),
            Some(space_id) => {
                let children = self.descendant_rooms(space_id);
                self.all_rooms
                    .iter()
                    .filter(|room| children.contains(room.id.as_ref()))
                    .cloned()
                    .collect::<Vec<Room>>()
                    .into()
            }
        };
        self.output.rooms(rooms);
    }

    pub(super) fn emit_spaces(&self) {
        let spaces: Vec<Space> = self
            .ordered_root_ids()
            .iter()
            .filter_map(|id| self.space(id))
            .map(|space| self.with_counts(space))
            .collect();
        self.output.spaces(spaces.into());
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
                    .filter_map(|child| self.space(child))
                    .map(|child| self.with_counts(child))
                    .collect()
            })
            .unwrap_or_default();
        self.output.subspaces(subspaces.into());
    }

    fn with_counts(&self, space: &Space) -> Space {
        let (unread, mentions) = self.space_counts.get(&space.id).copied().unwrap_or((0, 0));
        Space {
            unread,
            mentions,
            ..space.clone()
        }
    }

    fn room(&self, id: &str) -> Option<&Room> {
        self.room_index.get(id).and_then(|&i| self.all_rooms.get(i))
    }

    fn space(&self, id: &str) -> Option<&Space> {
        self.space_index.get(id).and_then(|&i| self.spaces.get(i))
    }

    fn rebuild_room_index(&mut self) {
        self.room_index = self
            .all_rooms
            .iter()
            .enumerate()
            .map(|(i, room)| (room.id.to_string(), i))
            .collect();
    }

    fn rebuild_space_index(&mut self) {
        self.space_index = self
            .spaces
            .iter()
            .enumerate()
            .map(|(i, space)| (space.id.clone(), i))
            .collect();
        let nested: HashSet<&str> = self
            .spaces
            .iter()
            .flat_map(|space| space.child_space_ids.iter().map(String::as_str))
            .collect();
        self.root_space_ids = self
            .spaces
            .iter()
            .filter(|space| !nested.contains(space.id.as_str()))
            .map(|space| space.id.clone())
            .collect();
    }

    fn rebuild_space_counts(&mut self) {
        let counts: HashMap<String, (u64, u64)> = self
            .spaces
            .iter()
            .map(|space| {
                let aggregate = self
                    .descendant_rooms(&space.id)
                    .iter()
                    .filter_map(|room_id| self.room(room_id))
                    .fold((0_u64, 0_u64), |(unread, mentions), room| {
                        (unread + room.unread_count, mentions + room.mention_count)
                    });
                (space.id.clone(), aggregate)
            })
            .collect();
        self.space_counts = counts;
    }

    fn descendant_rooms<'a>(&'a self, root: &'a str) -> HashSet<&'a str> {
        let mut rooms = HashSet::new();
        let mut visited = HashSet::new();
        let mut pending = vec![root];

        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let Some(space) = self.space(id) else {
                continue;
            };
            rooms.extend(space.child_room_ids.iter().map(String::as_str));
            pending.extend(space.child_space_ids.iter().map(String::as_str));
        }
        rooms
    }

    fn ordered_root_ids(&self) -> Vec<String> {
        let position: HashMap<&str, usize> = self
            .space_order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let mut ids = self.root_space_ids.clone();
        ids.sort_by_key(|id| position.get(id.as_str()).copied().unwrap_or(usize::MAX));
        ids
    }
}
