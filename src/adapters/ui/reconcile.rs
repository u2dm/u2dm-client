use std::collections::HashMap;
use std::sync::Arc;

use slint::{Model, VecModel};

use super::decode::forget_all_media_needs;
use super::dto::{prefetch_space_avatar, record_room_avatar_need};
use crate::domain::models::{EnrichmentDelta, Room, Space, TimelineMessage, TimelinePatch};
use crate::ports::media::MediaCache;

pub fn apply_timeline_patch<T: Clone + 'static>(
    model: &VecModel<T>,
    patch: TimelinePatch,
    convert: &dyn Fn(&TimelineMessage) -> T,
    enrich: &dyn Fn(&mut T, &EnrichmentDelta),
    entry_id: &dyn Fn(&T) -> &str,
) {
    let before = model.row_count();
    tracing::debug!(
        patch = patch.label(),
        model_rows_before = before,
        "apply_timeline_patch"
    );
    match patch {
        TimelinePatch::Reset(messages) => {
            forget_all_media_needs();
            let entries: Vec<T> = messages.iter().map(convert).collect();
            model.set_vec(entries);
        }
        TimelinePatch::Append(messages) => {
            for m in &messages {
                model.push(convert(m));
            }
        }
        TimelinePatch::PushFront(m) => {
            model.insert(0, convert(&m));
        }
        TimelinePatch::PushBack(m) => {
            model.push(convert(&m));
        }
        TimelinePatch::Insert { index, message } => {
            let idx = index.min(model.row_count());
            model.insert(idx, convert(&message));
        }
        TimelinePatch::Set { index, message } => {
            if index < model.row_count() {
                model.set_row_data(index, convert(&message));
            }
        }
        TimelinePatch::Remove { index } => {
            if index < model.row_count() {
                model.remove(index);
            }
        }
        TimelinePatch::PopFront => {
            if model.row_count() > 0 {
                model.remove(0);
            }
        }
        TimelinePatch::PopBack => {
            let count = model.row_count();
            if count > 0 {
                model.remove(count - 1);
            }
        }
        TimelinePatch::Truncate { length } => {
            while model.row_count() > length {
                model.remove(model.row_count() - 1);
            }
        }
        TimelinePatch::Clear => {
            forget_all_media_needs();
            model.set_vec(Vec::new());
        }
        TimelinePatch::Batch(patches) => {
            apply_batch(model, patches, convert, enrich, entry_id);
        }
        TimelinePatch::Enrich(delta) => {
            for i in 0..model.row_count() {
                if let Some(entry) = model.row_data(i)
                    && entry_id(&entry) == delta.unique_id.as_str()
                {
                    let mut updated = entry;
                    enrich(&mut updated, &delta);
                    model.set_row_data(i, updated);
                    break;
                }
            }
        }
    }
    tracing::debug!(
        model_rows_after = model.row_count(),
        "apply_timeline_patch done"
    );
}

fn apply_batch<T: Clone + 'static>(
    model: &VecModel<T>,
    patches: Vec<TimelinePatch>,
    convert: &dyn Fn(&TimelineMessage) -> T,
    enrich: &dyn Fn(&mut T, &EnrichmentDelta),
    entry_id: &dyn Fn(&T) -> &str,
) {
    for p in patches {
        apply_timeline_patch(model, p, convert, enrich, entry_id);
    }
}

pub fn reorder_rows<T: Clone + 'static>(model: &VecModel<T>, from: usize, to: usize) {
    if from < model.row_count() && to < model.row_count() {
        let entry = model.remove(from);
        model.insert(to, entry);
    }
}

pub fn apply_rooms<T: Clone + PartialEq + 'static>(
    model: &VecModel<T>,
    rooms: &[Arc<Room>],
    previous: &[Arc<Room>],
    media: &dyn MediaCache,
    convert: &dyn Fn(&Room) -> T,
    get_id: &dyn Fn(&T) -> &str,
) {
    for room in rooms {
        record_room_avatar_need(room, media);
    }
    apply_reconcile(
        model,
        rooms,
        previous,
        &|r| r.id.as_ref(),
        &|r| convert(r),
        get_id,
    );
}

pub(super) trait SameItem {
    fn same_item(&self, other: &Self) -> bool;
}

impl SameItem for Arc<Room> {
    fn same_item(&self, other: &Self) -> bool {
        Arc::ptr_eq(self, other) || **self == **other
    }
}

impl SameItem for Space {
    fn same_item(&self, other: &Self) -> bool {
        self == other
    }
}

pub fn apply_spaces<T: Clone + PartialEq + 'static>(
    model: &VecModel<T>,
    spaces: &[Space],
    media: &dyn MediaCache,
    convert: &dyn Fn(&Space) -> T,
    get_id: &dyn Fn(&T) -> &str,
) {
    for space in spaces {
        prefetch_space_avatar(space, media);
    }
    apply_reconcile(model, spaces, &[], &|s| s.id.as_str(), convert, get_id);
}

struct RowOps<'a, S, T> {
    item_id: &'a dyn Fn(&S) -> &str,
    row_id: &'a dyn Fn(&T) -> &str,
    item_to_row: &'a dyn Fn(&S) -> T,
}

#[derive(Default)]
struct ReconcileWork {
    moved: usize,
    inserted: usize,
    rebuilt: usize,
}

const NOT_IN_RUN: usize = usize::MAX;

type DestinationOfRow = Vec<usize>;
type RowsByDestination<T> = HashMap<usize, T>;
type IndexById<'a> = HashMap<&'a str, usize>;
type ItemById<'a, S> = HashMap<&'a str, &'a S>;

fn apply_reconcile<S: SameItem, T: Clone + PartialEq + 'static>(
    model: &VecModel<T>,
    items: &[S],
    previous: &[S],
    source_id: &dyn Fn(&S) -> &str,
    convert: &dyn Fn(&S) -> T,
    get_id: &dyn Fn(&T) -> &str,
) {
    let ops = RowOps {
        item_id: source_id,
        row_id: get_id,
        item_to_row: convert,
    };
    let destinations: IndexById<'_> = items
        .iter()
        .enumerate()
        .map(|(index, item)| (source_id(item), index))
        .collect();
    let unchanged_since: ItemById<'_, S> = previous
        .iter()
        .map(|item| (source_id(item), item))
        .collect();

    let mut destination_of_row = keep_one_row_per_item(model, &destinations, items.len(), &ops);
    let staying = longest_run_already_in_order(&destination_of_row);
    let lifted = lift_rows_that_must_move(model, &mut destination_of_row, &staying);

    let work = settle_rows_in_order(
        model,
        items,
        &mut destination_of_row,
        lifted,
        &unchanged_since,
        &ops,
    );

    while model.row_count() > items.len() {
        model.remove(model.row_count() - 1);
    }

    tracing::debug!(
        rows = items.len(),
        moved = work.moved,
        inserted = work.inserted,
        rebuilt = work.rebuilt,
        "apply_reconcile"
    );
}

fn keep_one_row_per_item<S, T: Clone + 'static>(
    model: &VecModel<T>,
    destinations: &IndexById<'_>,
    item_count: usize,
    ops: &RowOps<'_, S, T>,
) -> DestinationOfRow {
    let mut destination_of_row = DestinationOfRow::with_capacity(model.row_count());
    let mut already_kept = vec![false; item_count];
    let mut position = 0;

    while position < model.row_count() {
        let destination = model
            .row_data(position)
            .and_then(|row| destinations.get((ops.row_id)(&row)).copied())
            .filter(|destination| already_kept.get(*destination).is_some_and(|kept| !kept));

        match destination {
            Some(destination) => {
                if let Some(kept) = already_kept.get_mut(destination) {
                    *kept = true;
                }
                destination_of_row.push(destination);
                position += 1;
            }
            None => {
                model.remove(position);
            }
        }
    }

    destination_of_row
}

fn longest_run_already_in_order(destination_of_row: &[usize]) -> Vec<bool> {
    let mut stays = vec![false; destination_of_row.len()];
    let mut best_run_ending_at: Vec<usize> = Vec::new();
    let mut preceded_by: Vec<usize> = vec![NOT_IN_RUN; destination_of_row.len()];

    for (position, &destination) in destination_of_row.iter().enumerate() {
        let run_length = best_run_ending_at.partition_point(|end| {
            destination_of_row
                .get(*end)
                .is_some_and(|earlier| *earlier < destination)
        });

        if let Some(previous) = run_length
            .checked_sub(1)
            .and_then(|shorter| best_run_ending_at.get(shorter))
            && let Some(link) = preceded_by.get_mut(position)
        {
            *link = *previous;
        }

        match best_run_ending_at.get_mut(run_length) {
            Some(end) => *end = position,
            None => best_run_ending_at.push(position),
        }
    }

    let mut walk = best_run_ending_at.last().copied();
    while let Some(position) = walk {
        if let Some(stay) = stays.get_mut(position) {
            *stay = true;
        }
        walk = preceded_by
            .get(position)
            .copied()
            .filter(|link| *link != NOT_IN_RUN);
    }

    stays
}

fn lift_rows_that_must_move<T: Clone + 'static>(
    model: &VecModel<T>,
    destination_of_row: &mut DestinationOfRow,
    stays: &[bool],
) -> RowsByDestination<T> {
    let mut lifted = RowsByDestination::new();
    let rows = destination_of_row.len().min(model.row_count());

    for position in (0..rows).rev() {
        if stays.get(position).copied().unwrap_or(false) {
            continue;
        }
        let destination = destination_of_row.remove(position);
        lifted.insert(destination, model.remove(position));
    }

    lifted
}

fn settle_rows_in_order<S: SameItem, T: Clone + PartialEq + 'static>(
    model: &VecModel<T>,
    items: &[S],
    destination_of_row: &mut DestinationOfRow,
    mut lifted: RowsByDestination<T>,
    unchanged_since: &ItemById<'_, S>,
    ops: &RowOps<'_, S, T>,
) -> ReconcileWork {
    let mut work = ReconcileWork::default();

    for (destination, item) in items.iter().enumerate() {
        let unchanged = unchanged_since
            .get((ops.item_id)(item))
            .is_some_and(|before| before.same_item(item));
        let already_settled = destination_of_row.get(destination) == Some(&destination);

        if already_settled {
            if !unchanged {
                work.rebuilt += 1;
                rebuild_row(model, destination, item, ops);
            }
            continue;
        }

        let lifted_row = lifted.remove(&destination);
        if lifted_row.is_some() {
            work.moved += 1;
        } else {
            work.inserted += 1;
        }

        let row = lifted_row.filter(|_| unchanged).unwrap_or_else(|| {
            work.rebuilt += 1;
            (ops.item_to_row)(item)
        });

        if destination < model.row_count() {
            model.insert(destination, row);
        } else {
            model.push(row);
        }
        destination_of_row.insert(destination.min(destination_of_row.len()), destination);
    }

    work
}

fn rebuild_row<S, T: Clone + PartialEq + 'static>(
    model: &VecModel<T>,
    position: usize,
    item: &S,
    ops: &RowOps<'_, S, T>,
) {
    let rebuilt = (ops.item_to_row)(item);
    if model.row_data(position).as_ref() != Some(&rebuilt) {
        model.set_row_data(position, rebuilt);
    }
}
