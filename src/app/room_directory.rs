use std::collections::{HashMap, HashSet};
use std::mem;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::selection::Selection;
use super::task_group::TaskGroup;
use crate::commands::{DirectoryUpdate, UiCommand};
use crate::domain::models::{ConnectionStatus, Room, Space, SyncEvent};
use crate::ports::matrix::SyncPort;
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
        let root_indices = spaces
            .iter()
            .enumerate()
            .filter(|(_, space)| !nested.contains(space.id.as_str()))
            .map(|(i, _)| i)
            .collect();

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

pub(super) struct RoomDirectory {
    output: Arc<dyn AppOutputPort>,
    all_rooms: Arc<[Room]>,
    spaces: Arc<[Space]>,
    graph: SpaceGraph,
    counts: Vec<SpaceCounts>,
    spaces_dirty: bool,
    space_order: Vec<String>,
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
        self.spaces_dirty |= self.recompute_counts();
        true
    }

    pub(super) fn store_spaces(&mut self, spaces: Arc<[Space]>) -> bool {
        if !self.connected {
            return false;
        }
        self.spaces = spaces;
        self.graph = SpaceGraph::build(&self.spaces);
        self.recompute_counts();
        self.spaces_dirty = true;
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
        self.spaces = Arc::from(Vec::new());
        self.graph = SpaceGraph::default();
        self.counts.clear();
        self.spaces_dirty = false;
        self.space_order.clear();
    }

    pub(super) fn spawn_sync_pipeline(
        group: &mut TaskGroup,
        sync: Arc<dyn SyncPort>,
        output: Arc<dyn AppOutputPort>,
        cmd_tx: mpsc::UnboundedSender<UiCommand>,
        dir_in_tx: mpsc::UnboundedSender<DirectoryUpdate>,
    ) {
        let token = group.token();
        let on_sync: Box<dyn Fn(SyncEvent) + Send + Sync> = Box::new(move |event| match event {
            SyncEvent::Connected => {
                output.publish(Box::new(|view| {
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
                output.publish(Box::new(move |view| {
                    view.connection = ConnectionStatus::Error(msg);
                }));
            }
            SyncEvent::SessionExpired => {
                drop(cmd_tx.send(UiCommand::SessionExpired));
            }
        });

        group.spawn(async move {
            if let Err(e) = sync.start_sync(on_sync, token).await {
                tracing::error!("sync loop ended with error: {e}");
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
                    .cloned()
                    .collect::<Vec<Room>>()
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
        self.all_rooms.iter().find(|room| room.id.as_ref() == id)
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
        let position: HashMap<&str, usize> = self
            .space_order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let mut indices = self.graph.root_indices.clone();
        indices.sort_by_key(|&i| {
            self.spaces
                .get(i)
                .and_then(|space| position.get(space.id.as_str()).copied())
                .unwrap_or(usize::MAX)
        });
        indices
    }

    fn ordered_root_ids(&self) -> Vec<String> {
        self.ordered_root_indices()
            .into_iter()
            .filter_map(|i| self.spaces.get(i))
            .map(|space| space.id.clone())
            .collect()
    }
}
