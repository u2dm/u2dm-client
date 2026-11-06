use std::collections::{HashMap, HashSet};

use crate::domain::room::Space;

#[derive(Default)]
pub(super) struct SpaceGraph {
    pub(super) index: HashMap<String, usize>,
    pub(super) root_indices: Vec<usize>,
    room_ancestors: HashMap<String, Vec<usize>>,
}

impl SpaceGraph {
    pub(super) fn build(spaces: &[Space]) -> Self {
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

    pub(super) fn ancestors_of(&self, room_id: &str) -> &[usize] {
        self.room_ancestors
            .get(room_id)
            .map_or(&[] as &[usize], Vec::as_slice)
    }

    pub(super) fn contains_room(&self, space_index: usize, room_id: &str) -> bool {
        self.ancestors_of(room_id).contains(&space_index)
    }
}

pub(super) fn mark_reachable(
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
