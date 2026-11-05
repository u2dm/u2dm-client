use super::AppService;
use crate::app::event::{RoomActionEvent, RoomInfoEvent, SpaceIndexEvent};
use crate::app::space_index::ChildTarget;
use crate::domain::room::{NotifyMode, RoomId};

impl AppService {
    pub(super) fn open_space_index(&mut self) {
        if let Some(port) = self.port(|a| &a.space_index) {
            let target = self.listed_space();
            self.space_index.open(port, target);
        }
    }

    pub(super) fn page_space_index(&mut self) {
        if let Some(port) = self.port(|a| &a.space_index) {
            self.space_index.page(port);
        }
    }

    pub(super) fn retry_space_index(&mut self) {
        if let Some(port) = self.port(|a| &a.space_index) {
            self.space_index.retry(port);
        }
    }

    pub(super) fn follow_space_index(&mut self) {
        let Some(port) = self.port(|a| &a.space_index) else {
            return;
        };
        let target = self.listed_space();
        let membership = self.room_directory.membership(&self.selection);
        self.space_index.follow(port, target, &membership);
    }

    pub(super) fn refresh_space_index(&mut self) {
        if !self.space_index.is_tracking() {
            return;
        }
        let membership = self.room_directory.membership(&self.selection);
        self.space_index.membership_changed(&membership);
    }

    pub(super) fn handle_space_index(&mut self, event: SpaceIndexEvent) {
        match event {
            SpaceIndexEvent::Paged {
                generation,
                outcome,
            } => {
                let Some(port) = self.port(|a| &a.space_index) else {
                    return;
                };
                let membership = self.room_directory.membership(&self.selection);
                self.space_index
                    .fetched(port, generation, outcome, &membership);
            }
            SpaceIndexEvent::AvatarsReady { generation, ready } => {
                self.space_index.avatars_ready(generation, ready);
            }
            SpaceIndexEvent::ChildJoinSettled {
                room_id,
                name,
                outcome,
            } => {
                let membership = self.room_directory.membership(&self.selection);
                self.space_index
                    .join_settled(room_id, &name, outcome, &membership);
            }
        }
    }

    pub(super) fn join_space_child(&mut self, room_id: RoomId) {
        let Some(port) = self.port(|a| &a.space_index) else {
            return;
        };
        let membership = self.room_directory.membership(&self.selection);
        self.space_index.join(port, room_id, &membership);
    }

    pub(super) async fn open_space_child(&mut self, room_id: RoomId) {
        let membership = self.room_directory.membership(&self.selection);
        let target = self.space_index.open_target(&room_id, &membership);
        match target {
            Some(ChildTarget::Room) => self.select_room(room_id).await,
            Some(ChildTarget::Subspace) => self.handle_select_subspace(Some(room_id)),
            None => {
                tracing::debug!(%room_id, "ignoring an open for a space index row that cannot open");
            }
        }
    }

    pub(super) fn open_room_info(&mut self, room_id: &RoomId) {
        if self.selection.room.as_ref() != Some(room_id) {
            tracing::debug!(%room_id, "ignoring room info for a room that is not selected");
            return;
        }
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info.open(port, self.room_directory.room(room_id));
        }
    }

    pub(super) fn show_room_info_pane(&mut self) {
        let Some(port) = self.port(|a| &a.room_info) else {
            return;
        };
        let selected = self
            .selection
            .room
            .as_ref()
            .and_then(|room_id| self.room_directory.room(room_id));
        self.room_info.show_pane(port, selected);
    }

    pub(super) fn retarget_room_info(&mut self, room_id: &RoomId) {
        match self.port(|a| &a.room_info) {
            Some(port) => self
                .room_info
                .follow_selection(port, self.room_directory.room(room_id)),
            None => self.room_info.close(),
        }
    }

    pub(super) fn page_room_members(&mut self) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info.page(port);
        }
    }

    pub(super) fn retry_room_members(&mut self) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info.retry(port);
        }
    }

    pub(super) fn filter_room_members(&mut self, query: String) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info.filter(port, query);
        }
    }

    pub(super) fn set_room_notify(&mut self, room_id: &RoomId, mode: NotifyMode) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info
                .set_notify(port, self.room_directory.room(room_id), mode);
        }
    }

    pub(super) fn mark_room_read(&mut self, room_id: &RoomId) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info
                .mark_read(port, self.room_directory.room(room_id));
        }
    }

    pub(super) fn copy_room_link(&mut self, room_id: &RoomId) {
        let name = self
            .room_directory
            .room(room_id)
            .map(|room| room.display_name.clone())
            .or_else(|| {
                self.room_directory
                    .space_name(room_id)
                    .map(ToOwned::to_owned)
            });
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info.copy_link(port, room_id, name);
        }
    }

    pub(super) fn open_room_log(&mut self, room_id: RoomId) {
        let name = self
            .room_directory
            .room(&room_id)
            .map(|room| room.display_name.clone())
            .unwrap_or_default();
        self.room_log.open(room_id, name);
    }

    pub(super) fn follow_room_info(&mut self) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info.follow(port, self.room_directory.rooms());
        }
    }

    pub(super) fn handle_room_info(&mut self, event: RoomInfoEvent) {
        match event {
            RoomInfoEvent::RosterFetched {
                generation,
                outcome,
            } => {
                if let Some(port) = self.port(|a| &a.room_info) {
                    self.room_info.roster_fetched(port, generation, outcome);
                }
            }
            RoomInfoEvent::AboutFetched { generation, about } => {
                self.room_info.about_fetched(generation, about);
            }
            RoomInfoEvent::AvatarsReady { generation, ready } => {
                self.room_info.avatars_ready(generation, ready);
            }
        }
    }

    pub(super) fn handle_room_action(&mut self, event: RoomActionEvent) {
        match event {
            RoomActionEvent::NotifySettled {
                request,
                room_id,
                name,
                outcome,
            } => {
                let listed = self.room_directory.room(&room_id).map(|room| room.notify);
                self.room_info
                    .notify_settled(request, room_id, &name, outcome, listed);
            }
            RoomActionEvent::LeaveSettled {
                room_id,
                name,
                outcome,
            } => {
                self.room_info.leave_settled(&room_id, &name, outcome);
            }
            RoomActionEvent::ReadSettled {
                room_id,
                name,
                outcome,
            } => {
                self.room_info.read_settled(&room_id, &name, outcome);
            }
            RoomActionEvent::LinkResolved {
                request,
                name,
                link,
            } => {
                self.room_info.link_resolved(request, &name, link);
            }
        }
    }
}
