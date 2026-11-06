use std::sync::Arc;

use super::RoomDirectory;
use super::rail::RailPlace;
use crate::app::selection::{RoomFilter, Selection};
use crate::commands::view::SpaceMatch;
use crate::domain::departure::Removal;
use crate::domain::invite::Invite;
use crate::domain::room::{Room, RoomId};

impl RoomDirectory {
    pub(super) fn listed<T: Listed>(&self, all: &Arc<[Arc<T>]>, sel: &Selection) -> Arc<[Arc<T>]> {
        match &sel.filter {
            RoomFilter::All if self.query.is_empty() => Arc::clone(all),
            RoomFilter::All => kept(all, |item| self.query.matches(item.title())),
            RoomFilter::Direct => kept(all, |item| {
                item.is_direct() && self.query.matches(item.title())
            }),
            RoomFilter::Space { space, subspace } => {
                let space_id = subspace.as_ref().unwrap_or(space);
                match self.graph.index.get(space_id.as_ref()).copied() {
                    Some(space_index) => kept(all, |item| {
                        self.graph.contains_room(space_index, item.id().as_ref())
                    }),
                    None => Arc::from(Vec::new()),
                }
            }
        }
    }

    pub(in crate::app) fn lists(&self, sel: &Selection, room_id: &RoomId) -> bool {
        self.listed(&self.listable, sel)
            .iter()
            .any(|room| room.id == *room_id)
    }

    pub(super) fn space_matches(&self, sel: &Selection) -> Arc<[SpaceMatch]> {
        if self.query.is_empty() || !matches!(sel.filter, RoomFilter::All) {
            return Arc::from(Vec::new());
        }
        self.rail_places()
            .into_iter()
            .filter_map(|place| self.space_match(place))
            .collect::<Vec<SpaceMatch>>()
            .into()
    }

    fn space_match(&self, place: RailPlace) -> Option<SpaceMatch> {
        let space = self
            .spaces
            .get(place.space)
            .filter(|space| self.query.matches(&space.name))?;
        Some(SpaceMatch {
            id: space.id.clone(),
            name: space.name.clone(),
            avatar_mxc: space.avatar_mxc.clone(),
            parent: place
                .parent
                .and_then(|parent| self.spaces.get(parent))
                .map(|parent| parent.name.clone()),
            rooms: self.joined_rooms_in(place.space),
            flags: self.flags.get(place.space).copied().unwrap_or_default(),
        })
    }

    fn joined_rooms_in(&self, space_index: usize) -> usize {
        self.listable
            .iter()
            .filter(|room| self.graph.contains_room(space_index, room.id.as_ref()))
            .count()
    }
}

pub(super) trait Listed {
    fn id(&self) -> &RoomId;
    fn title(&self) -> &str;
    fn is_direct(&self) -> bool;
}

impl Listed for Room {
    fn id(&self) -> &RoomId {
        &self.id
    }

    fn title(&self) -> &str {
        &self.display_name
    }

    fn is_direct(&self) -> bool {
        self.is_direct
    }
}

impl Listed for Invite {
    fn id(&self) -> &RoomId {
        &self.id
    }

    fn title(&self) -> &str {
        &self.name
    }

    fn is_direct(&self) -> bool {
        Invite::is_direct(self)
    }
}

impl Listed for Removal {
    fn id(&self) -> &RoomId {
        &self.id
    }

    fn title(&self) -> &str {
        &self.name
    }

    fn is_direct(&self) -> bool {
        self.is_direct
    }
}

pub(super) fn kept<T>(all: &[Arc<T>], keep: impl Fn(&T) -> bool) -> Arc<[Arc<T>]> {
    all.iter()
        .filter(|item| keep(item))
        .map(Arc::clone)
        .collect::<Vec<Arc<T>>>()
        .into()
}
