use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use slint::{Image, Model, VecModel};

use super::{SameItem, apply_reconcile};
use crate::adapters::ui::backend::UiBackend;
use crate::adapters::ui::decode::ShownAvatar;
use crate::adapters::ui::dto::{
    INVITES_HEADING_ROW, ROOMS_HEADING_ROW, RoomRowKind, SPACES_HEADING_ROW, heading_to_dto,
    invite_to_dto, record_invite_avatar_need, record_removal_avatar_need, record_room_avatar_need,
    removal_to_dto, request_space_match_avatar, room_to_dto, space_match_to_dto,
};
use crate::adapters::ui::fields::RoomFields;
use crate::commands::view::{DirectoryView, InvitesSection, SpaceMatch};
use crate::domain::departure::Removal;
use crate::domain::invite::Invite;
use crate::domain::room::Room;
use crate::ports::media::MediaCache;

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
