use std::collections::{HashMap, HashSet};
use std::mem;
use std::sync::Arc;

use slint::{Image, Model, SharedString, VecModel};

use super::backend::UiBackend;
use super::decode::{ShownAvatar, forget_all_media_needs};
use super::dto::{
    INVITED_HEADING_ROW, INVITES_HEADING_ROW, PackGrid, PackRowDto, ROOMS_HEADING_ROW, RoomRowKind,
    SPACES_HEADING_ROW, heading_to_dto, invite_to_dto, prefetch_space_avatar,
    record_invite_avatar_need, record_removal_avatar_need, record_room_avatar_need, removal_to_dto,
    request_invitee_avatar, request_member_avatar, request_mention_avatar, request_reader_avatar,
    request_space_child_avatar, request_space_match_avatar, room_to_dto, space_match_to_dto,
};
use super::fields::RoomFields;
use super::richtext::forget_styled_bodies;
use super::session::with_session;
use super::splice_model::SpliceModel;
use super::spoilers::forget_spoiler_rows;
use crate::commands::view::{
    CallTile, DirectoryView, InviteeRow, InvitesSection, RosterRow, SpaceIndexRow, SpaceMatch,
};
use crate::domain::departure::Removal;
use crate::domain::image_pack::PackOrigin;
use crate::domain::invite::Invite;
use crate::domain::message::TimelineMessage;
use crate::domain::room::{Room, Space};
use crate::domain::room_info::{Reader, RosterMember};
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

    fn shows_same_media(&self, unique_id: &str, message: &TimelineMessage) -> bool {
        unique_id == message.unique_id
            && self.fingerprint_of.get(unique_id) == Some(&message.enrichment_fingerprint())
    }

    fn row_for(&self, delta: &EnrichmentDelta) -> Option<usize> {
        self.row_of
            .get(&delta.unique_id)
            .copied()
            .filter(|_| self.holds_revision(delta))
    }
}

#[derive(Default)]
pub struct PackIndex {
    row_of_cell: HashMap<String, usize>,
    row_of_pack: HashMap<String, usize>,
    row_shapes: Vec<PackRowShape>,
    awaited_downloads: HashMap<String, String>,
    pack_avatars: HashMap<String, PackAvatarWatch>,
}

struct PackAvatarWatch {
    downloading_mxc: Option<String>,
    fallback_cell_key: String,
}

#[derive(PartialEq)]
struct PackRowShape {
    is_header: bool,
    title: SharedString,
    untitled: bool,
    pack_id: SharedString,
    origin: PackOrigin,
    cell_keys: Vec<SharedString>,
}

impl PackRowShape {
    fn of(row: &PackRowDto) -> Self {
        Self {
            is_header: row.is_header,
            title: row.title.clone(),
            untitled: row.untitled,
            pack_id: row.pack_id.clone(),
            origin: row.origin,
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

pub fn index_pack_grid(grid: &PackGrid) -> RowReplacements {
    with_session(|session| {
        let index = &mut session.pack_index;
        let shapes: Vec<PackRowShape> = grid.rows.iter().map(PackRowShape::of).collect();
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
        index.pack_avatars.clear();
        for (row, pack) in grid.packs.iter().enumerate() {
            index.row_of_pack.insert(pack.id.to_string(), row);
            if let Some(avatar) = &pack.avatar {
                let watch = PackAvatarWatch {
                    downloading_mxc: avatar.downloading_mxc.clone(),
                    fallback_cell_key: avatar.fallback_cell_key.to_string(),
                };
                index.pack_avatars.insert(pack.id.to_string(), watch);
            }
        }
        index.row_shapes = shapes;
        replaced
    })
}

pub fn retain_awaited_downloads(mut still_awaited: impl FnMut(&str, &str) -> bool) {
    let mut awaited = with_session(|session| mem::take(&mut session.pack_index.awaited_downloads));
    awaited.retain(|key, mxc| still_awaited(key, mxc));
    with_session(|session| session.pack_index.awaited_downloads = awaited);
}

pub fn shows_pack_avatar(pack_id: &str) -> bool {
    with_session(|session| session.pack_index.pack_avatars.contains_key(pack_id))
}

pub fn forget_pack_avatar(pack_id: &str) -> Option<String> {
    with_session(|session| session.pack_index.pack_avatars.remove(pack_id))
        .map(|watch| watch.fallback_cell_key)
}

pub fn downloading_pack_avatars() -> Vec<(String, String)> {
    with_session(|session| {
        session
            .pack_index
            .pack_avatars
            .iter()
            .filter_map(|(pack, watch)| Some((pack.clone(), watch.downloading_mxc.clone()?)))
            .collect()
    })
}

pub fn pack_avatar_downloaded(pack_id: &str) {
    with_session(|session| {
        if let Some(watch) = session.pack_index.pack_avatars.get_mut(pack_id) {
            watch.downloading_mxc = None;
        }
    });
}

pub fn pack_cell_row(key: &str) -> Option<usize> {
    with_session(|session| session.pack_index.row_of_cell.get(key).copied())
}

pub fn pack_tab_row(pack_id: &str) -> Option<usize> {
    with_session(|session| session.pack_index.row_of_pack.get(pack_id).copied())
}

pub fn timeline_row_of(unique_id: &str) -> Option<usize> {
    with_session(|session| session.timeline_index.row_of.get(unique_id).copied())
}

struct TimelineOps<'a, T> {
    convert: &'a dyn Fn(&TimelineMessage) -> T,
    keep_shown: &'a dyn Fn(&T, &mut T),
    enrich: &'a dyn Fn(&mut T, &EnrichmentDelta),
    entry_id: &'a dyn Fn(&T) -> &str,
}

pub fn apply_timeline_patch<T: Clone + 'static>(
    model: &SpliceModel<T>,
    patch: TimelinePatch,
    convert: &dyn Fn(&TimelineMessage) -> T,
    keep_shown: &dyn Fn(&T, &mut T),
    enrich: &dyn Fn(&mut T, &EnrichmentDelta),
    entry_id: &dyn Fn(&T) -> &str,
) {
    let ops = TimelineOps {
        convert,
        keep_shown,
        enrich,
        entry_id,
    };
    let mut index = with_session(|session| mem::take(&mut session.timeline_index));
    apply_patch(model, patch, &mut index, &ops);
    with_session(|session| session.timeline_index = index);
}

fn forget_row_caches() {
    forget_all_media_needs();
    forget_styled_bodies();
    forget_spoiler_rows();
}

fn apply_patch<T: Clone + 'static>(
    model: &SpliceModel<T>,
    patch: TimelinePatch,
    index: &mut TimelineIndex,
    ops: &TimelineOps<'_, T>,
) {
    let TimelineOps {
        convert,
        enrich,
        entry_id,
        ..
    } = *ops;
    let before = model.row_count();
    tracing::debug!(
        patch = patch.label(),
        model_rows_before = before,
        "apply_timeline_patch"
    );
    match patch {
        TimelinePatch::Reset(messages) => {
            forget_row_caches();
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
            set_row(model, at, before, index, &message, ops);
        }
        TimelinePatch::Remove { index: at } => remove_row(model, at, before, index, entry_id),
        TimelinePatch::PopFront => remove_row(model, 0, before, index, entry_id),
        TimelinePatch::PopBack => {
            remove_row(model, before.saturating_sub(1), before, index, entry_id);
        }
        TimelinePatch::Truncate { length } => truncate_rows(model, length, index),
        TimelinePatch::Clear => {
            forget_row_caches();
            index.clear();
            model.set_vec(Vec::new());
        }
        TimelinePatch::Batch(patches) => apply_batch(model, patches, index, ops),
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
    ops: &TimelineOps<'_, T>,
) {
    let mut edges = EdgeInsertions::default();
    for patch in patches {
        if let Some(positional) = edges.absorb(patch) {
            edges.flush(model, index, ops.convert);
            apply_patch(model, positional, index, ops);
        }
    }
    edges.flush(model, index, ops.convert);
}

fn set_row<T: Clone + 'static>(
    model: &SpliceModel<T>,
    row: usize,
    row_count: usize,
    index: &mut TimelineIndex,
    message: &TimelineMessage,
    ops: &TimelineOps<'_, T>,
) {
    if row >= row_count {
        return;
    }
    let shown = model.row_data(row);
    let shown_id = shown.as_ref().map(|entry| (ops.entry_id)(entry).to_owned());
    let same_media = shown_id
        .as_deref()
        .is_some_and(|id| index.shows_same_media(id, message));
    index.replaced_at(row, shown_id.as_deref(), message);
    let mut rebuilt = (ops.convert)(message);
    if same_media && let Some(shown) = &shown {
        (ops.keep_shown)(shown, &mut rebuilt);
    }
    model.set_row_data(row, rebuilt);
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

#[derive(Clone, Copy, Default)]
pub struct RoomListing<'a> {
    pub invites: &'a [Arc<Invite>],
    pub invites_section: InvitesSection,
    pub rooms: &'a [Arc<Room>],
    pub removals: &'a [Arc<Removal>],
    pub spaces: &'a [SpaceMatch],
}

pub struct ListedPlace {
    pub row: usize,
    pub headings_above: usize,
}

impl<'a> RoomListing<'a> {
    pub fn of(directory: &'a DirectoryView) -> Self {
        Self {
            invites: &directory.invites,
            invites_section: directory.invites_section,
            rooms: &directory.rooms,
            removals: &directory.removals,
            spaces: &directory.space_matches,
        }
    }

    fn shown_invites(self) -> &'a [Arc<Invite>] {
        match self.invites_section {
            InvitesSection::Folded => &[],
            InvitesSection::Unfolded | InvitesSection::Matches => self.invites,
        }
    }

    fn rows(self) -> Vec<ListedRow<'a>> {
        let invites = self.shown_invites();
        let has_rooms = !self.rooms.is_empty() || !self.removals.is_empty();
        let invites_heading =
            (!self.invites.is_empty()).then_some(ListedRow::Heading(Heading::Invites {
                section: self.invites_section,
                count: self.invites.len(),
            }));
        let rooms_heading =
            (!invites.is_empty() && has_rooms).then_some(ListedRow::Heading(Heading::Rooms));
        let spaces_heading =
            (!self.spaces.is_empty()).then_some(ListedRow::Heading(Heading::Spaces));
        invites_heading
            .into_iter()
            .chain(invites.iter().map(ListedRow::Invite))
            .chain(rooms_heading)
            .chain(self.rooms_by_activity())
            .chain(spaces_heading)
            .chain(self.spaces.iter().map(ListedRow::Space))
            .collect()
    }

    fn rooms_by_activity(self) -> Vec<ListedRow<'a>> {
        let mut rows = Vec::with_capacity(self.rooms.len() + self.removals.len());
        let mut rooms = self.rooms.iter().peekable();
        let mut removals = self.removals.iter().peekable();
        loop {
            let removal_first = match (rooms.peek(), removals.peek()) {
                (Some(room), Some(removal)) => removal.removed_ts > room.last_activity_ts,
                (None, Some(_)) => true,
                (Some(_), None) => false,
                (None, None) => break,
            };
            let row = if removal_first {
                removals.next().map(ListedRow::Removal)
            } else {
                rooms.next().map(ListedRow::Room)
            };
            rows.extend(row);
        }
        rows
    }

    pub fn place_of(self, room_id: &str) -> Option<ListedPlace> {
        let rows = self.rows();
        let row = rows.iter().position(
            |listed| matches!(listed, ListedRow::Room(room) if room.id.as_ref() == room_id),
        )?;
        let headings_above = rows
            .iter()
            .take(row)
            .filter(|listed| matches!(listed, ListedRow::Heading(_)))
            .count();
        Some(ListedPlace {
            row,
            headings_above,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Heading {
    Invites {
        section: InvitesSection,
        count: usize,
    },
    Rooms,
    Spaces,
}

impl Heading {
    fn id(self) -> &'static str {
        match self {
            Self::Invites { .. } => INVITES_HEADING_ROW,
            Self::Rooms => ROOMS_HEADING_ROW,
            Self::Spaces => SPACES_HEADING_ROW,
        }
    }

    fn kind(self) -> RoomRowKind {
        match self {
            Self::Invites { section, .. } => match section {
                InvitesSection::Folded => RoomRowKind::InvitesFolded,
                InvitesSection::Unfolded => RoomRowKind::InvitesUnfolded,
                InvitesSection::Matches => RoomRowKind::InvitesHeading,
            },
            Self::Rooms => RoomRowKind::RoomsHeading,
            Self::Spaces => RoomRowKind::SpacesHeading,
        }
    }

    fn invites(self) -> usize {
        match self {
            Self::Invites { count, .. } => count,
            Self::Rooms | Self::Spaces => 0,
        }
    }
}

#[derive(Clone, Copy)]
enum ListedRow<'a> {
    Invite(&'a Arc<Invite>),
    Room(&'a Arc<Room>),
    Removal(&'a Arc<Removal>),
    Heading(Heading),
    Space(&'a SpaceMatch),
}

impl ListedRow<'_> {
    fn id(&self) -> &str {
        match self {
            Self::Invite(invite) => invite.id.as_ref(),
            Self::Room(room) => room.id.as_ref(),
            Self::Removal(removal) => removal.id.as_ref(),
            Self::Heading(heading) => heading.id(),
            Self::Space(space) => &space.id,
        }
    }

    fn to_row<B: UiBackend>(self, media: &dyn MediaCache) -> B::Room {
        match self {
            Self::Invite(invite) => invite_to_dto(invite, media).into(),
            Self::Room(room) => room_to_dto(room, media).into(),
            Self::Removal(removal) => removal_to_dto(removal, media).into(),
            Self::Heading(heading) => {
                heading_to_dto(heading.id(), heading.kind(), heading.invites()).into()
            }
            Self::Space(space) => space_match_to_dto(space, media).into(),
        }
    }
}

impl SameItem for ListedRow<'_> {
    fn same_item(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Invite(invite), Self::Invite(other)) => invite.same_item(other),
            (Self::Removal(removal), Self::Removal(other)) => removal.same_item(other),
            (Self::Room(room), Self::Room(other)) => room.same_item(other),
            (Self::Heading(heading), Self::Heading(other)) => heading == other,
            (Self::Space(space), Self::Space(other)) => space == other,
            _ => false,
        }
    }
}

pub fn apply_rooms<B: UiBackend>(
    model: &VecModel<B::Room>,
    listing: RoomListing<'_>,
    previous: RoomListing<'_>,
    media: &dyn MediaCache,
) {
    let mut still_expected = HashSet::new();
    for invite in listing.shown_invites() {
        if record_invite_avatar_need(invite, media) == ShownAvatar::StillExpected {
            still_expected.insert(invite.id.as_ref());
        }
    }
    for room in listing.rooms {
        if record_room_avatar_need(room, media) == ShownAvatar::StillExpected {
            still_expected.insert(room.id.as_ref());
        }
    }
    for removal in listing.removals {
        if record_removal_avatar_need(removal, media) == ShownAvatar::StillExpected {
            still_expected.insert(removal.id.as_ref());
        }
    }
    for space in listing.spaces {
        if request_space_match_avatar(space, media) == ShownAvatar::StillExpected {
            still_expected.insert(space.id.as_str());
        }
    }
    let shown = avatars_still_shown::<B>(model, &still_expected);
    apply_reconcile(
        model,
        &listing.rows(),
        &previous.rows(),
        &ListedRow::id,
        &|row| keep_shown_avatar::<B>(row.to_row::<B>(media), &shown),
        &|entry| entry.id(),
    );
}

fn avatars_still_shown<'a, B: UiBackend>(
    model: &VecModel<B::Room>,
    still_expected: &HashSet<&'a str>,
) -> HashMap<&'a str, Image> {
    model
        .iter()
        .filter(B::Room::has_avatar)
        .filter_map(|row| still_expected.get(row.id()).map(|id| (*id, row.avatar())))
        .collect()
}

fn keep_shown_avatar<B: UiBackend>(mut row: B::Room, shown: &HashMap<&str, Image>) -> B::Room {
    if !row.has_avatar()
        && let Some(image) = shown.get(row.id())
    {
        row.set_avatar(image.clone());
        row.set_has_avatar(true);
    }
    row
}

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
