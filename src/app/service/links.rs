use super::AppService;
use crate::app::event::PreviewEvent;
use crate::app::lifecycle;
use crate::domain::matrix_link::{AddressEntry, MatrixLink, RoomAddress, RoomLink};
use crate::domain::room::RoomId;
use crate::domain::timeline::TimelineFocus;

enum LinkTarget {
    Room(RoomId),
    Invite(RoomId),
    Space(RoomId),
}

impl AppService {
    pub(super) async fn route_room_link(&mut self, link: RoomLink) {
        tracing::info!(room = %link.room, event = ?link.event, "routing a room link");
        if let Some(target) = self.local_target(&link.room) {
            self.open_local(target, link.focus()).await;
            return;
        }
        let Some(port) = self.port(|a| &a.room_previews) else {
            return;
        };
        self.close_user_info();
        self.event_source.close();
        self.readers.close();
        self.invites.close();
        let membership = self.room_directory.membership(&self.selection);
        self.room_previews.show(port, link, &membership);
    }

    pub(super) async fn open_external_link(&mut self, link: MatrixLink) {
        self.room_previews.hold(link);
        self.release_held_link().await;
    }

    pub(super) async fn release_held_link(&mut self) {
        let ready = self.lifecycle.phase() == lifecycle::AppPhase::Syncing
            && self.room_directory.rooms_landed();
        if !ready {
            return;
        }
        match self.room_previews.take_held() {
            Some(MatrixLink::User(user_id)) => self.open_user_info(user_id),
            Some(MatrixLink::Room(link)) => self.route_room_link(link).await,
            None => {}
        }
    }

    pub(super) async fn find_room(&mut self, entry: AddressEntry) {
        match entry {
            AddressEntry::Room(link) => self.route_room_link(link).await,
            AddressEntry::Unrecognized => self.room_previews.refuse_address(),
        }
    }

    fn local_target(&self, room: &RoomAddress) -> Option<LinkTarget> {
        let room_id = match room {
            RoomAddress::Id(room_id) => room_id.clone(),
            RoomAddress::Alias(alias) => self.room_directory.room_by_alias(alias)?.id.clone(),
        };
        if self.room_directory.room(&room_id).is_some() {
            return Some(LinkTarget::Room(room_id));
        }
        if self.room_directory.invite(&room_id).is_some() {
            return Some(LinkTarget::Invite(room_id));
        }
        self.room_directory
            .rail_route(&room_id)
            .map(|_| LinkTarget::Space(room_id))
    }

    async fn open_local(&mut self, target: LinkTarget, focus: TimelineFocus) {
        match target {
            LinkTarget::Room(room_id) => self.open_linked_room(room_id, focus).await,
            LinkTarget::Invite(room_id) => {
                self.room_previews.close();
                self.invites.open(self.room_directory.invite(&room_id));
            }
            LinkTarget::Space(space_id) => {
                self.room_previews.close();
                self.open_space(&space_id);
            }
        }
    }

    pub(super) async fn open_linked_room(&mut self, room_id: RoomId, focus: TimelineFocus) {
        if self.selection.room.as_ref() == Some(&room_id) {
            self.room_previews.close();
            self.room_info.close_dialog();
            self.close_user_info();
            if let TimelineFocus::Event(event_id) = focus {
                self.active_timeline.jump_to_event(event_id);
            }
            return;
        }
        if !self.room_directory.lists(&self.selection, &room_id) {
            self.handle_select_space(None);
        }
        self.select_room_at(room_id, focus).await;
    }

    async fn open_previewed(&mut self, resolved: Option<RoomLink>) {
        let Some(link) = resolved else {
            return;
        };
        let Some(target) = self.local_target(&link.room) else {
            tracing::debug!(room = %link.room, "the linked room is not listed yet");
            return;
        };
        self.open_local(target, link.focus()).await;
    }

    pub(super) async fn follow_previews(&mut self) {
        let membership = self.room_directory.membership(&self.selection);
        let resolved = self.room_previews.follow(&membership);
        self.open_previewed(resolved).await;
    }

    pub(super) fn join_previewed_room(&mut self, address: &str) {
        let Some(port) = self.port(|a| &a.room_previews) else {
            return;
        };
        let membership = self.room_directory.membership(&self.selection);
        self.room_previews.join(port, address, &membership);
    }

    pub(super) fn retry_room_preview(&mut self) {
        if let Some(port) = self.port(|a| &a.room_previews) {
            self.room_previews.retry(port);
        }
    }

    pub(super) async fn handle_preview(&mut self, event: PreviewEvent) {
        let port = self.port(|a| &a.room_previews);
        let membership = self.room_directory.membership(&self.selection);
        let resolved = match event {
            PreviewEvent::Landed {
                generation,
                outcome,
            } => port.and_then(|port| {
                self.room_previews
                    .landed(port, generation, outcome, &membership)
            }),
            PreviewEvent::AvatarsReady { generation, ready } => {
                self.room_previews.avatars_ready(generation, ready);
                None
            }
            PreviewEvent::JoinSettled {
                room,
                name,
                outcome,
            } => self
                .room_previews
                .join_settled(&room, &name, outcome, &membership),
        };
        self.open_previewed(resolved).await;
    }
}
