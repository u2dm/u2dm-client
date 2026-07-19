use crate::domain::models::RoomId;

#[derive(Default)]
pub(super) struct Selection {
    pub(super) space: Option<RoomId>,
    pub(super) subspace: Option<RoomId>,
    pub(super) room: Option<RoomId>,
    pub(super) generation: i32,
}

impl Selection {
    pub(super) fn next_generation(&mut self) -> i32 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    pub(super) fn set_space(&mut self, space: Option<RoomId>) {
        self.space = space.filter(|id| !id.is_empty());
        self.subspace = None;
    }

    pub(super) fn set_subspace(&mut self, subspace: Option<RoomId>) {
        self.subspace = subspace.filter(|id| !id.is_empty());
    }

    pub(super) fn active_filter(&self) -> Option<&RoomId> {
        self.subspace.as_ref().or(self.space.as_ref())
    }

    pub(super) fn space_id_str(&self) -> String {
        self.space.as_deref().unwrap_or_default().to_owned()
    }

    pub(super) fn subspace_id_str(&self) -> String {
        self.subspace.as_deref().unwrap_or_default().to_owned()
    }
}
