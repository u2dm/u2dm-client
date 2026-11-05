use super::AppService;
use crate::app::event::{ActionOutcome, CreationSettled};
use crate::app::invites::{Answer, Arrival};
use crate::app::room_creation::Created;
use crate::app::upgrades::{Toward, UpgradeJoined};
use crate::domain::room::RoomId;
use crate::domain::room_creation::{RoomDraft, RoomKind};
use crate::domain::timeline::TimelineFocus;
use crate::domain::user_info::server_part;

impl AppService {
    pub(super) async fn follow_invites(&mut self) {
        let membership = self.room_directory.membership(&self.selection);
        let arrival = self
            .invites
            .follow(self.room_directory.invites(), &membership);
        self.open_arrival(arrival).await;
    }

    pub(super) fn answer_invite(&mut self, room_id: &RoomId, answer: Answer) {
        if let Some(port) = self.port(|a| &a.invites) {
            self.invites
                .answer(port, self.room_directory.invite(room_id), answer);
        }
    }

    pub(super) async fn settle_invite_answer(
        &mut self,
        room_id: &RoomId,
        name: &str,
        answer: Answer,
        outcome: ActionOutcome,
    ) {
        let membership = self.room_directory.membership(&self.selection);
        let arrival = self
            .invites
            .answered(room_id, name, answer, outcome, &membership);
        self.open_arrival(arrival).await;
    }

    pub(super) fn join_upgrade_link(&mut self, room_id: &RoomId, toward: Toward) {
        let Some(port) = self.port(|a| &a.upgrades) else {
            return;
        };
        let membership = self.room_directory.membership(&self.selection);
        self.upgrades
            .join(port, toward, self.room_directory.room(room_id), &membership);
        self.refresh_upgrades();
    }

    pub(super) async fn settle_upgrade_join(&mut self, joined: UpgradeJoined) {
        let membership = self.room_directory.membership(&self.selection);
        let arrival = self
            .upgrades
            .settled(joined, &membership, self.selection.room.as_ref());
        self.open_upgraded(arrival).await;
    }

    pub(super) async fn follow_upgrades(&mut self) {
        let membership = self.room_directory.membership(&self.selection);
        let arrival = self
            .upgrades
            .follow(&membership, self.selection.room.as_ref());
        self.open_upgraded(arrival).await;
    }

    async fn open_upgraded(&mut self, arrival: Option<RoomId>) {
        match arrival {
            Some(room_id) => self.select_room(room_id).await,
            None => self.refresh_upgrades(),
        }
    }

    pub(super) fn refresh_upgrades(&mut self) {
        let selected = self
            .selection
            .room
            .as_ref()
            .and_then(|room_id| self.room_directory.room(room_id));
        let membership = self.room_directory.membership(&self.selection);
        self.upgrades.refresh(selected, &membership);
    }

    async fn open_arrival(&mut self, arrival: Option<Arrival>) {
        match arrival {
            Some(Arrival::Room(room_id)) => self.select_room(room_id).await,
            Some(Arrival::Space(space_id)) => self.open_space(&space_id),
            None => {}
        }
    }

    pub(super) fn open_room_creation(&mut self, kind: RoomKind) {
        let Some(running) = self.held_session.running() else {
            return;
        };
        let server = server_part(&running.session.user_id).to_owned();
        let parent = self
            .listed_space()
            .filter(|space| self.room_directory.space_powers(&space.id).add_children);
        self.room_creation.open(kind, parent, server);
    }

    pub(super) fn create_room(&mut self, draft: &RoomDraft) {
        if let Some(port) = self.port(|a| &a.room_creation) {
            self.room_creation.create(port, draft);
        }
    }

    pub(super) async fn settle_room_creation(&mut self, settled: CreationSettled) {
        let membership = self.room_directory.membership(&self.selection);
        let created = self.room_creation.settled(settled, &membership);
        self.open_created(created).await;
    }

    pub(super) async fn follow_creations(&mut self) {
        let membership = self.room_directory.membership(&self.selection);
        let created = self.room_creation.follow(&membership);
        self.open_created(created).await;
    }

    async fn open_created(&mut self, created: Option<Created>) {
        let Some(created) = created else {
            return;
        };
        match created.kind {
            RoomKind::Room => {
                self.open_linked_room(created.room_id.clone(), TimelineFocus::ReadPosition)
                    .await;
            }
            RoomKind::Space => self.open_space(&created.room_id),
        }
        self.open_invite_people(&created.room_id);
    }
}
