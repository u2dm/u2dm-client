use std::collections::HashMap;
use std::mem;
use std::sync::Arc;

use slint::{Model, SharedString, VecModel};

use super::decode::forget_all_media_needs;
use super::dto::{StickerGrid, StickerRowDto, prefetch_space_avatar, record_room_avatar_need};
use super::richtext::forget_styled_bodies;
use super::session::with_session;
use super::splice_model::SpliceModel;
use crate::domain::message::TimelineMessage;
use crate::domain::room::{Room, Space};
use crate::domain::timeline::{EnrichmentDelta, TimelinePatch};
use crate::ports::media::MediaCache;

#[derive(Default)]
pub struct TimelineIndex {
    row_of: HashMap<String, usize>,
    fingerprint_of: HashMap<String, u64>,
}

impl TimelineIndex {
    fn clear(&mut self) {
        self.row_of.clear();
        self.fingerprint_of.clear();
    }

    fn reset(&mut self, messages: &[TimelineMessage]) {
        self.clear();
        self.extend_from(0, messages);
    }

    fn extend_from(&mut self, base: usize, messages: &[TimelineMessage]) {
        for (offset, message) in messages.iter().enumerate() {
            self.remember(base.saturating_add(offset), message);
        }
    }

    fn remember(&mut self, row: usize, message: &TimelineMessage) {
        self.row_of.insert(message.unique_id.clone(), row);
        self.fingerprint_of
            .insert(message.unique_id.clone(), message.enrichment_fingerprint());
    }

    fn forget(&mut self, unique_id: &str) {
        self.row_of.remove(unique_id);
        self.fingerprint_of.remove(unique_id);
    }

    fn spliced_at(&mut self, row: usize, messages: &[TimelineMessage]) {
        let count = messages.len();
        for existing in self.row_of.values_mut() {
            if *existing >= row {
                *existing = existing.saturating_add(count);
            }
        }
        self.extend_from(row, messages);
    }

    fn replaced_at(&mut self, row: usize, previous: Option<&str>, message: &TimelineMessage) {
        if let Some(previous) = previous.filter(|id| *id != message.unique_id) {
            self.forget(previous);
        }
        self.remember(row, message);
    }

    fn removed_at(&mut self, row: usize, unique_id: Option<&str>) {
        if let Some(unique_id) = unique_id {
            self.forget(unique_id);
        }
        for existing in self.row_of.values_mut() {
            if *existing > row {
                *existing = existing.saturating_sub(1);
            }
        }
    }

    fn truncated_to(&mut self, length: usize) {
        let Self {
            row_of,
            fingerprint_of,
        } = self;
        row_of.retain(|_, row| *row < length);
        fingerprint_of.retain(|unique_id, _| row_of.contains_key(unique_id));
    }

    fn holds_revision(&self, delta: &EnrichmentDelta) -> bool {
        self.fingerprint_of.get(&delta.unique_id) == Some(&delta.fingerprint)
    }

    fn row_for(&self, delta: &EnrichmentDelta) -> Option<usize> {
        self.row_of
            .get(&delta.unique_id)
            .copied()
            .filter(|_| self.holds_revision(delta))
    }
}

#[derive(Default)]
pub struct StickerIndex {
    row_of_cell: HashMap<String, usize>,
    row_of_pack: HashMap<String, usize>,
    row_shapes: Vec<StickerRowShape>,
    awaited_downloads: HashMap<String, String>,
}

#[derive(PartialEq)]
struct StickerRowShape {
    is_header: bool,
    title: SharedString,
    cell_keys: Vec<SharedString>,
}

impl StickerRowShape {
    fn of(row: &StickerRowDto) -> Self {
        Self {
            is_header: row.is_header,
            title: row.title.clone(),
            cell_keys: row.cells.iter().map(|cell| cell.key.clone()).collect(),
        }
    }
}

pub struct RowReplacements(Vec<bool>);

impl RowReplacements {
    pub fn must_replace(&self, row: usize) -> bool {
        self.0.get(row).copied().unwrap_or(true)
    }
}

pub fn index_sticker_grid(grid: &StickerGrid) -> RowReplacements {
    with_session(|session| {
        let index = &mut session.sticker_index;
        let shapes: Vec<StickerRowShape> = grid.rows.iter().map(StickerRowShape::of).collect();
        let replaced = RowReplacements(
            shapes
                .iter()
                .enumerate()
                .map(|(row, shape)| index.row_shapes.get(row) != Some(shape))
                .collect(),
        );
        let mut previously_awaited = mem::take(&mut index.awaited_downloads);
        index.row_of_cell.clear();
        index.row_of_pack.clear();
        for (row, entry) in grid.rows.iter().enumerate() {
            for cell in &entry.cells {
                index.row_of_cell.insert(cell.key.to_string(), row);
                let awaited = if replaced.must_replace(row) {
                    cell.awaited_mxc.clone()
                } else {
                    previously_awaited.remove(cell.key.as_str())
                };
                if let Some(mxc) = awaited {
                    index.awaited_downloads.insert(cell.key.to_string(), mxc);
                }
            }
        }
        for (row, pack) in grid.packs.iter().enumerate() {
            index.row_of_pack.insert(pack.id.to_string(), row);
        }
        index.row_shapes = shapes;
        replaced
    })
}

pub fn retain_awaited_downloads(mut still_awaited: impl FnMut(&str, &str) -> bool) {
    let mut awaited =
        with_session(|session| mem::take(&mut session.sticker_index.awaited_downloads));
    awaited.retain(|key, mxc| still_awaited(key, mxc));
    with_session(|session| session.sticker_index.awaited_downloads = awaited);
}

pub fn sticker_cell_row(key: &str) -> Option<usize> {
    with_session(|session| session.sticker_index.row_of_cell.get(key).copied())
}

pub fn sticker_pack_row(pack_id: &str) -> Option<usize> {
    with_session(|session| session.sticker_index.row_of_pack.get(pack_id).copied())
}

pub fn timeline_row_of(unique_id: &str) -> Option<usize> {
    with_session(|session| session.timeline_index.row_of.get(unique_id).copied())
}

pub fn apply_timeline_patch<T: Clone + 'static>(
    model: &SpliceModel<T>,
    patch: TimelinePatch,
    convert: &dyn Fn(&TimelineMessage) -> T,
    enrich: &dyn Fn(&mut T, &EnrichmentDelta),
    entry_id: &dyn Fn(&T) -> &str,
) {
    let mut index = with_session(|session| mem::take(&mut session.timeline_index));
    apply_patch(model, patch, &mut index, convert, enrich, entry_id);
    with_session(|session| session.timeline_index = index);
}

fn apply_patch<T: Clone + 'static>(
    model: &SpliceModel<T>,
    patch: TimelinePatch,
    index: &mut TimelineIndex,
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
            forget_styled_bodies();
            index.reset(&messages);
            let entries: Vec<T> = messages.iter().map(convert).collect();
            model.set_vec(entries);
        }
        TimelinePatch::Append(messages) => splice_rows(model, before, &messages, index, convert),
        TimelinePatch::PushFront(m) => splice_rows(model, 0, &[m], index, convert),
        TimelinePatch::PushBack(m) => splice_rows(model, before, &[m], index, convert),
        TimelinePatch::Insert { index: at, message } => {
            splice_rows(model, at.min(before), &[message], index, convert);
        }
        TimelinePatch::Set { index: at, message } => {
            set_row(model, at, before, index, &message, convert, entry_id);
        }
        TimelinePatch::Remove { index: at } => remove_row(model, at, before, index, entry_id),
        TimelinePatch::PopFront => remove_row(model, 0, before, index, entry_id),
        TimelinePatch::PopBack => {
            remove_row(model, before.saturating_sub(1), before, index, entry_id);
        }
        TimelinePatch::Truncate { length } => truncate_rows(model, length, index),
        TimelinePatch::Clear => {
            forget_all_media_needs();
            forget_styled_bodies();
            index.clear();
            model.set_vec(Vec::new());
        }
        TimelinePatch::Batch(patches) => {
            apply_batch(model, patches, index, convert, enrich, entry_id);
        }
        TimelinePatch::Enrich(delta) => enrich_target(model, &delta, index, enrich, entry_id),
    }
    tracing::debug!(
        model_rows_after = model.row_count(),
        "apply_timeline_patch done"
    );
}

fn id_at<T: Clone + 'static>(
    model: &SpliceModel<T>,
    row: usize,
    entry_id: &dyn Fn(&T) -> &str,
) -> Option<String> {
    model.row_data(row).map(|entry| entry_id(&entry).to_owned())
}

fn splice_rows<T: Clone + 'static>(
    model: &SpliceModel<T>,
    row: usize,
    messages: &[TimelineMessage],
    index: &mut TimelineIndex,
    convert: &dyn Fn(&TimelineMessage) -> T,
) {
    if messages.is_empty() {
        return;
    }
    if row < model.row_count() {
        index.spliced_at(row, messages);
    } else {
        index.extend_from(row, messages);
    }
    model.insert_rows(row, messages.iter().map(convert).collect());
}

#[derive(Default)]
struct EdgeInsertions {
    newest_first_at_front: Vec<TimelineMessage>,
    at_back: Vec<TimelineMessage>,
}

impl EdgeInsertions {
    fn absorb(&mut self, patch: TimelinePatch) -> Option<TimelinePatch> {
        match patch {
            TimelinePatch::PushFront(message) | TimelinePatch::Insert { index: 0, message } => {
                self.newest_first_at_front.push(message);
            }
            TimelinePatch::PushBack(message) => self.at_back.push(message),
            TimelinePatch::Append(messages) => self.at_back.extend(messages),
            other => return Some(other),
        }
        None
    }

    fn flush<T: Clone + 'static>(
        &mut self,
        model: &SpliceModel<T>,
        index: &mut TimelineIndex,
        convert: &dyn Fn(&TimelineMessage) -> T,
    ) {
        let mut front = mem::take(&mut self.newest_first_at_front);
        front.reverse();
        splice_rows(model, 0, &front, index, convert);
        let back = mem::take(&mut self.at_back);
        splice_rows(model, model.row_count(), &back, index, convert);
    }
}

fn apply_batch<T: Clone + 'static>(
    model: &SpliceModel<T>,
    patches: Vec<TimelinePatch>,
    index: &mut TimelineIndex,
    convert: &dyn Fn(&TimelineMessage) -> T,
    enrich: &dyn Fn(&mut T, &EnrichmentDelta),
    entry_id: &dyn Fn(&T) -> &str,
) {
    let mut edges = EdgeInsertions::default();
    for patch in patches {
        if let Some(positional) = edges.absorb(patch) {
            edges.flush(model, index, convert);
            apply_patch(model, positional, index, convert, enrich, entry_id);
        }
    }
    edges.flush(model, index, convert);
}

#[allow(clippy::too_many_arguments)]
fn set_row<T: Clone + 'static>(
    model: &SpliceModel<T>,
    row: usize,
    row_count: usize,
    index: &mut TimelineIndex,
    message: &TimelineMessage,
    convert: &dyn Fn(&TimelineMessage) -> T,
    entry_id: &dyn Fn(&T) -> &str,
) {
    if row >= row_count {
        return;
    }
    index.replaced_at(row, id_at(model, row, entry_id).as_deref(), message);
    model.set_row_data(row, convert(message));
}

fn remove_row<T: Clone + 'static>(
    model: &SpliceModel<T>,
    row: usize,
    row_count: usize,
    index: &mut TimelineIndex,
    entry_id: &dyn Fn(&T) -> &str,
) {
    if row >= row_count {
        return;
    }
    index.removed_at(row, id_at(model, row, entry_id).as_deref());
    model.remove(row);
}

fn truncate_rows<T: Clone + 'static>(
    model: &SpliceModel<T>,
    length: usize,
    index: &mut TimelineIndex,
) {
    index.truncated_to(length);
    model.truncate(length);
}

fn enrich_target<T: Clone + 'static>(
    model: &SpliceModel<T>,
    delta: &EnrichmentDelta,
    index: &TimelineIndex,
    enrich: &dyn Fn(&mut T, &EnrichmentDelta),
    entry_id: &dyn Fn(&T) -> &str,
) {
    let Some(row) = index.row_for(delta) else {
        tracing::debug!(
            unique_id = delta.unique_id,
            "dropped an enrichment delta with no live row at its revision"
        );
        return;
    };
    enrich_row(model, row, delta, enrich, entry_id);
}

fn enrich_row<T: Clone + 'static>(
    model: &SpliceModel<T>,
    row: usize,
    delta: &EnrichmentDelta,
    enrich: &dyn Fn(&mut T, &EnrichmentDelta),
    entry_id: &dyn Fn(&T) -> &str,
) {
    let Some(mut entry) = model.row_data(row) else {
        return;
    };
    if entry_id(&entry) != delta.unique_id.as_str() {
        tracing::warn!(
            row,
            unique_id = delta.unique_id,
            "the timeline index disagrees with the model, dropping an enrichment delta"
        );
        return;
    }
    enrich(&mut entry, delta);
    model.set_row_data(row, entry);
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
