use std::collections::HashSet;
use std::sync::Arc;

use super::RoomDirectory;
use crate::app::selection::Selection;
use crate::commands::view::SpaceHeading;
use crate::domain::room::{RoomId, Space, UnreadFlags};

pub(in crate::app) struct RailRoute {
    pub(in crate::app) space: RoomId,
    pub(in crate::app) subspace: Option<RoomId>,
}

#[derive(Clone, Copy)]
pub(super) struct RailPlace {
    pub(super) space: usize,
    pub(super) parent: Option<usize>,
}

impl RoomDirectory {
    pub(in crate::app) fn rail_route(&self, id: &RoomId) -> Option<RailRoute> {
        let index = self.graph.index.get(id.as_ref()).copied()?;
        let place = self
            .rail_places()
            .into_iter()
            .find(|place| place.space == index)?;
        let route = match place.parent.and_then(|parent| self.spaces.get(parent)) {
            Some(parent) => RailRoute {
                space: RoomId::new(parent.id.clone()),
                subspace: Some(id.clone()),
            },
            None => RailRoute {
                space: id.clone(),
                subspace: None,
            },
        };
        Some(route)
    }

    pub(super) fn rail_places(&self) -> Vec<RailPlace> {
        let roots = self.ordered_root_indices();
        let mut claimed: HashSet<usize> = roots.iter().copied().collect();
        let mut places = Vec::with_capacity(self.spaces.len());
        for root in roots {
            places.push(RailPlace {
                space: root,
                parent: None,
            });
            for child in self.child_space_indices(root) {
                if claimed.insert(child) {
                    places.push(RailPlace {
                        space: child,
                        parent: Some(root),
                    });
                }
            }
        }
        places
    }

    fn child_space_indices(&self, space_index: usize) -> impl Iterator<Item = usize> + '_ {
        self.spaces
            .get(space_index)
            .into_iter()
            .flat_map(|space| space.child_space_ids.iter())
            .filter_map(|id| self.graph.index.get(id).copied())
    }

    pub(super) fn emit_spaces(&self) {
        let spaces: Vec<Space> = self
            .ordered_root_indices()
            .into_iter()
            .filter_map(|i| self.space_with_flags(i))
            .collect();
        let spaces: Arc<[Space]> = spaces.into();
        self.output
            .publish(Box::new(move |view| view.directory.spaces = spaces));
    }

    pub(in crate::app) fn emit_subspaces(&self, sel: &Selection) {
        let subspaces: Vec<Space> = sel
            .space()
            .and_then(|id| self.space(id))
            .map(|space| {
                space
                    .child_space_ids
                    .iter()
                    .filter_map(|child| self.graph.index.get(child).copied())
                    .filter_map(|i| self.space_with_flags(i))
                    .collect()
            })
            .unwrap_or_default();
        let subspaces: Arc<[Space]> = subspaces.into();
        self.output
            .publish(Box::new(move |view| view.directory.subspaces = subspaces));
    }

    pub(in crate::app) fn emit_listed_space(&self, sel: &Selection) {
        let heading = sel
            .listed_space()
            .and_then(|id| self.space(id))
            .map(|space| SpaceHeading {
                id: space.id.clone(),
                name: space.name.clone(),
                member_count: space.member_count,
                may_invite: space.powers.invite,
                may_add_children: space.powers.add_children,
            })
            .unwrap_or_default();
        let space_name = sel
            .space()
            .and_then(|id| self.space_name(id))
            .unwrap_or_default()
            .to_owned();
        self.output.publish(Box::new(move |view| {
            view.directory.listed_space = heading;
            view.directory.space_name = space_name;
        }));
    }

    pub(super) fn emit_rail_flags(&self) {
        let flags = self.direct_flags;
        let invites_pending = !self.invites.is_empty();
        let attention = self.attention;
        self.output.publish(Box::new(move |view| {
            view.directory.direct_flags = flags;
            view.directory.invites_pending = invites_pending;
            view.directory.attention = attention;
        }));
    }

    fn space_with_flags(&self, space_index: usize) -> Option<Space> {
        let space = self.spaces.get(space_index)?;
        let flags = self.flags.get(space_index).copied().unwrap_or_default();
        Some(Space {
            alert: flags.alert,
            mention: flags.mention,
            hint: flags.hint,
            ..space.clone()
        })
    }

    pub(super) fn recompute_flags(&mut self) -> bool {
        let mut next = vec![UnreadFlags::default(); self.spaces.len()];
        let mut direct = UnreadFlags::default();
        for room in self.listable.iter() {
            for &i in self.graph.ancestors_of(room.id.as_ref()) {
                let Some(slot) = next.get_mut(i) else {
                    continue;
                };
                slot.absorb(room);
            }
            if room.is_direct {
                direct.absorb(room);
            }
        }
        for invite in self.invites.iter() {
            for &i in self.graph.ancestors_of(invite.id.as_ref()) {
                if let Some(slot) = next.get_mut(i) {
                    slot.absorb_invite();
                }
            }
            if invite.is_direct() {
                direct.absorb_invite();
            }
        }
        let attention =
            self.listable.iter().filter(|room| room.alert()).count() + self.invites.len();
        let changed =
            next != self.flags || direct != self.direct_flags || attention != self.attention;
        self.flags = next;
        self.direct_flags = direct;
        self.attention = attention;
        changed
    }
}
