mod packs;
mod rooms;
mod timeline;

use std::collections::HashMap;
use std::sync::Arc;

use slint::{Model, VecModel};

pub(super) use self::packs::{
    PackIndex, RowReplacements, downloading_pack_avatars, forget_pack_avatar, index_pack_grid,
    pack_avatar_downloaded, pack_cell_row, pack_tab_row, retain_awaited_downloads,
    shows_pack_avatar,
};
pub(super) use self::rooms::{RoomListing, apply_rooms};
pub(super) use self::timeline::{
    RowTime, TimelineIndex, apply_timeline_patch, reorder_rows, restamp_rows, timeline_row_of,
};
use super::dto::{
    INVITED_HEADING_ROW, prefetch_space_avatar, request_invitee_avatar, request_member_avatar,
    request_mention_avatar, request_reader_avatar, request_space_child_avatar,
};
use crate::commands::view::{CallTile, InviteeRow, RosterRow, SpaceIndexRow};
use crate::domain::room::Space;
use crate::domain::room_info::{Reader, RosterMember};
use crate::ports::media::MediaCache;

pub(super) trait SameItem {
    fn same_item(&self, other: &Self) -> bool;
}

impl<T: PartialEq> SameItem for Arc<T> {
    fn same_item(&self, other: &Self) -> bool {
        Arc::ptr_eq(self, other) || **self == **other
    }
}

impl SameItem for Space {
    fn same_item(&self, other: &Self) -> bool {
        self == other
    }
}

impl SameItem for SpaceIndexRow {
    fn same_item(&self, other: &Self) -> bool {
        self == other
    }
}

impl SameItem for RosterRow {
    fn same_item(&self, other: &Self) -> bool {
        self == other
    }
}

impl SameItem for InviteeRow {
    fn same_item(&self, other: &Self) -> bool {
        self == other
    }
}

pub(super) trait RowItem: SameItem {
    fn item_id(&self) -> &str;
    fn request_avatar(&self, media: &dyn MediaCache);
}

impl RowItem for Space {
    fn item_id(&self) -> &str {
        self.id.as_str()
    }

    fn request_avatar(&self, media: &dyn MediaCache) {
        prefetch_space_avatar(self, media);
    }
}

impl RowItem for RosterRow {
    fn item_id(&self) -> &str {
        match self {
            Self::Member(member) => &member.user_id,
            Self::InvitedHeading => INVITED_HEADING_ROW,
        }
    }

    fn request_avatar(&self, media: &dyn MediaCache) {
        request_member_avatar(self, media);
    }
}

impl RowItem for Arc<Reader> {
    fn item_id(&self) -> &str {
        self.user_id.as_str()
    }

    fn request_avatar(&self, media: &dyn MediaCache) {
        request_reader_avatar(self, media);
    }
}

impl RowItem for Arc<RosterMember> {
    fn item_id(&self) -> &str {
        self.user_id.as_str()
    }

    fn request_avatar(&self, media: &dyn MediaCache) {
        request_mention_avatar(self, media);
    }
}

impl RowItem for InviteeRow {
    fn item_id(&self) -> &str {
        self.person.user_id.as_ref()
    }

    fn request_avatar(&self, media: &dyn MediaCache) {
        request_invitee_avatar(self, media);
    }
}

impl RowItem for SpaceIndexRow {
    fn item_id(&self) -> &str {
        self.child.id.as_ref()
    }

    fn request_avatar(&self, media: &dyn MediaCache) {
        request_space_child_avatar(self, media);
    }
}

pub fn apply_rows<S: RowItem, T: Clone + PartialEq + 'static>(
    model: &VecModel<T>,
    items: &[S],
    previous: &[S],
    media: &dyn MediaCache,
    convert: &dyn Fn(&S) -> T,
    get_id: &dyn Fn(&T) -> &str,
) {
    for item in items {
        item.request_avatar(media);
    }
    apply_reconcile(model, items, previous, &S::item_id, convert, get_id);
}

pub fn rows_to_redo<'a, T>(
    shown: Option<(&'a Arc<[T]>, usize)>,
    rows: &Arc<[T]>,
    avatars_ready: usize,
) -> Option<&'a [T]> {
    let rows_changed = shown.is_none_or(|(shown, _)| !Arc::ptr_eq(shown, rows));
    let avatars_landed = shown.is_some_and(|(_, ready)| ready != avatars_ready);
    if !rows_changed && !avatars_landed {
        return None;
    }
    Some(
        shown
            .filter(|_| !avatars_landed)
            .map_or(&[] as &[_], |(shown, _)| shown.as_ref()),
    )
}

struct KeyedTile<'a> {
    key: String,
    tile: &'a CallTile,
}

impl SameItem for KeyedTile<'_> {
    fn same_item(&self, other: &Self) -> bool {
        self.tile == other.tile
    }
}

fn keyed_tiles(tiles: &[CallTile]) -> Vec<KeyedTile<'_>> {
    tiles
        .iter()
        .map(|tile| KeyedTile {
            key: tile.key().to_string(),
            tile,
        })
        .collect()
}

pub fn apply_call_tiles<T: Clone + PartialEq + 'static>(
    model: &VecModel<T>,
    tiles: &[CallTile],
    previous: &[CallTile],
    convert: &dyn Fn(&CallTile) -> T,
    get_key: &dyn Fn(&T) -> &str,
) {
    apply_reconcile(
        model,
        &keyed_tiles(tiles),
        &keyed_tiles(previous),
        &|keyed| keyed.key.as_str(),
        &|keyed| convert(keyed.tile),
        get_key,
    );
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
