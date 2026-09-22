use crate::commands::view::RoomScope;
use crate::domain::room::RoomId;

#[derive(Default)]
pub(super) enum RoomFilter {
    #[default]
    All,
    Direct,
    Space {
        space: RoomId,
        subspace: Option<RoomId>,
    },
}

#[derive(Default)]
pub(super) struct Selection {
    pub(super) filter: RoomFilter,
    pub(super) room: Option<RoomId>,
    pub(super) generation: i32,
}

impl Selection {
    pub(super) fn next_generation(&mut self) -> i32 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    pub(super) fn set_space(&mut self, space: Option<RoomId>) {
        self.filter = match space.filter(|id| !id.is_empty()) {
            Some(space) => RoomFilter::Space {
                space,
                subspace: None,
            },
            None => RoomFilter::All,
        };
    }

    pub(super) fn set_direct(&mut self) {
        self.filter = RoomFilter::Direct;
    }

    pub(super) fn set_subspace(&mut self, subspace: Option<RoomId>) {
        if let RoomFilter::Space {
            subspace: current, ..
        } = &mut self.filter
        {
            *current = subspace.filter(|id| !id.is_empty());
        }
    }

    pub(super) fn space(&self) -> Option<&RoomId> {
        match &self.filter {
            RoomFilter::Space { space, .. } => Some(space),
            RoomFilter::All | RoomFilter::Direct => None,
        }
    }

    pub(super) fn subspace(&self) -> Option<&RoomId> {
        match &self.filter {
            RoomFilter::Space { subspace, .. } => subspace.as_ref(),
            RoomFilter::All | RoomFilter::Direct => None,
        }
    }

    pub(super) fn listed_space(&self) -> Option<&RoomId> {
        self.subspace().or_else(|| self.space())
    }

    pub(super) fn scope(&self) -> RoomScope {
        match self.filter {
            RoomFilter::All => RoomScope::All,
            RoomFilter::Direct => RoomScope::Direct,
            RoomFilter::Space { .. } => RoomScope::Space,
        }
    }

    pub(super) fn space_id_str(&self) -> String {
        self.space().map(ToString::to_string).unwrap_or_default()
    }

    pub(super) fn subspace_id_str(&self) -> String {
        self.subspace().map(ToString::to_string).unwrap_or_default()
    }
}
