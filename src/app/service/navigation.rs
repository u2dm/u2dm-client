use super::{AppService, EmittedRoom};
use crate::app::pinned::PinDirection;
use crate::app::room_directory::{InvitesFold, RoomDirectory, RoomMeta};
use crate::app::show_toast;
use crate::app::space_index::ListedSpace;
use crate::commands::effects::Effect;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::UiCommand;
use crate::commands::view::Toast;
use crate::domain::push::PushNotice;
use crate::domain::room::RoomId;
use crate::domain::room_search::TitleQuery;
use crate::domain::timeline::TimelineFocus;

impl AppService {
    pub(super) async fn jump_to_latest(&mut self, room_id: RoomId, generation: i32) {
        if self.active_timeline.is_live() {
            self.active_timeline.jump_to_latest(&room_id, generation);
        } else if self.active_timeline.is_current(&room_id, generation) {
            self.open_room(room_id, TimelineFocus::Latest).await;
        }
    }

    pub(super) async fn refocus_timeline(
        &mut self,
        room_id: RoomId,
        generation: i32,
        focus: TimelineFocus,
    ) {
        if self.active_timeline.is_current(&room_id, generation) {
            self.open_room(room_id, focus).await;
        }
    }

    pub(super) async fn return_to_live_for_send(&mut self, room_id: RoomId) {
        if !self.active_timeline.is_live() && self.active_timeline.is_active_room(&room_id) {
            self.open_room(room_id, TimelineFocus::Latest).await;
        }
    }

    pub(super) fn publish_selection(&self) {
        let scope = self.selection.scope();
        let space_id = self.selection.space_id_str();
        let subspace_id = self.selection.subspace_id_str();
        self.output.publish(Box::new(move |view| {
            view.directory.scope = scope;
            view.directory.space_id = space_id;
            view.directory.subspace_id = subspace_id;
        }));
        self.room_directory.emit_listed_space(&self.selection);
    }

    async fn emit_selected_room(&mut self, room: EmittedRoom) {
        let effect = Effect::SelectedRoom {
            id: room.id.clone(),
            name: room.meta.name.clone(),
            avatar_mxc: room.meta.avatar_mxc.clone(),
            member_count: room.meta.member_count,
            encrypted: room.meta.encrypted,
            event_ids: room.meta.event_ids,
            readable_by_anyone: room.meta.readable_by_anyone,
            polls: room.meta.polls,
            messages: room.meta.messages,
            generation: room.generation,
            live: room.live,
        };
        self.last_selected_room = Some(room);
        self.output.emit(effect).await;
    }

    pub(super) fn move_space(&mut self, from: usize, to: usize) {
        let Some(space_order) = self.port(|a| &a.space_order) else {
            return;
        };
        if let Some(write) = self.room_directory.move_space(from, to) {
            RoomDirectory::spawn_order_write(
                &mut self.operations,
                space_order,
                write,
                self.events.clone(),
            );
        }
    }

    pub(super) fn revert_space_orders(&mut self, op: u64, spaces: &[String], error: &str) {
        if !self.room_directory.rollback_space_orders(op, spaces) {
            return;
        }
        tracing::warn!(op, "reverting optimistic space order: {error}");
        show_toast(
            self.output.as_ref(),
            Toast::Error(UserMessage::new(UserMessageKind::SpaceOrderSaveFailed)),
        );
    }

    pub(super) fn handle_select_space(&mut self, space: Option<RoomId>) {
        self.selection.set_space(space);
        self.show_selected_scope();
        self.follow_space_index();
    }

    pub(super) fn handle_select_direct(&mut self) {
        self.selection.set_direct();
        self.show_selected_scope();
        self.follow_space_index();
    }

    fn show_selected_scope(&self) {
        self.publish_selection();
        self.room_directory.emit_subspaces(&self.selection);
        self.room_directory.emit_rooms(&self.selection);
    }

    pub(super) fn handle_select_subspace(&mut self, subspace: Option<RoomId>) {
        self.selection.set_subspace(subspace);
        self.publish_selection();
        self.room_directory.emit_rooms(&self.selection);
        self.follow_space_index();
    }

    pub(super) fn filter_rooms(&mut self, query: &str) {
        if self.room_directory.search(TitleQuery::new(query)) {
            self.room_directory.emit_rooms(&self.selection);
        }
    }

    pub(super) fn fold_invites(&mut self, fold: InvitesFold) {
        if self.room_directory.fold_invites(fold) {
            self.room_directory.emit_rooms(&self.selection);
        }
    }

    pub(super) fn open_space(&mut self, space: &RoomId) {
        let Some(route) = self.room_directory.rail_route(space) else {
            tracing::debug!(%space, "ignoring an open for a space the rail does not show");
            return;
        };
        self.selection.set_space(Some(route.space));
        self.selection.set_subspace(route.subspace);
        self.show_selected_scope();
        self.follow_space_index();
    }

    pub(super) fn listed_space(&self) -> Option<ListedSpace> {
        let id = self.selection.listed_space()?.clone();
        let name = self
            .room_directory
            .space_name(&id)
            .unwrap_or_default()
            .to_owned();
        Some(ListedSpace { id, name })
    }

    pub(super) async fn select_room(&mut self, room_id: RoomId) {
        self.select_room_at(room_id, TimelineFocus::ReadPosition)
            .await;
    }

    pub(super) async fn select_room_at(&mut self, room_id: RoomId, focus: TimelineFocus) {
        if let Some(removal) = self.room_directory.removal(&room_id) {
            tracing::debug!(%room_id, "you were removed from the room; explaining that instead of opening it");
            self.removals.open(Some(removal));
            return;
        }
        self.space_index.close();
        self.invites.close();
        self.room_previews.close();
        self.invite_people.close();
        self.upgrades.retarget(&room_id);
        self.retarget_room_info(&room_id);
        self.user_info.close();
        self.event_source.close();
        self.readers.close();
        self.mentions.reset();
        self.sync_selected_room(Some(&room_id));
        self.follow_pinned(room_id.clone());
        self.follow_identity_alerts(room_id.clone());
        self.follow_typing(room_id.clone());
        self.open_room(room_id, focus).await;
        self.follow_presence();
        self.follow_calls();
        self.notifications
            .on_screen(self.active_timeline.on_screen());
    }

    pub(super) async fn open_room_from(&mut self, opening: UiCommand) {
        match opening {
            UiCommand::SelectRoom(room_id) => self.select_room(room_id).await,
            UiCommand::OpenNotification(room_id) => self.open_notification(room_id).await,
            UiCommand::OpenRoomLink(link) => self.route_room_link(link).await,
            UiCommand::OpenExternalLink(link) => self.open_external_link(link).await,
            UiCommand::FindRoom(entry) => self.find_room(entry).await,
            other => tracing::warn!(command = %other, "this command opens no room"),
        }
    }

    async fn open_notification(&mut self, room_id: RoomId) {
        self.notifications.on_screen(Some(&room_id));
        if self.room_directory.invite(&room_id).is_some() {
            self.invites.open(self.room_directory.invite(&room_id));
            return;
        }
        if self.selection.room.as_ref() == Some(&room_id) {
            return;
        }
        if !self.room_directory.lists(&self.selection, &room_id) {
            self.handle_select_space(None);
        }
        self.select_room(room_id).await;
    }

    pub(super) fn settle_notice(&mut self, session: u64, notice: PushNotice) {
        if !self.lifecycle.runs(session) {
            return;
        }
        if notice.is_call() && self.calls.rings_in(&notice.room_id) {
            tracing::debug!(room_id = %notice.room_id, "the room rings; its call notice shows no message notification");
            return;
        }
        self.notifications.arrived(notice, &self.room_directory);
    }

    fn follow_typing(&mut self, room_id: RoomId) {
        if let Some(typing) = self.port(|a| &a.typing) {
            self.typing.retarget(typing, room_id);
        }
    }

    fn follow_pinned(&mut self, room_id: RoomId) {
        if let Some(pinned) = self.port(|a| &a.pinned) {
            self.pinned.follow(pinned, room_id);
        }
    }

    pub(super) fn open_pinned(&mut self, event_id: String) {
        let Some(room_id) = self.active_timeline.room_id() else {
            return;
        };
        if !self.pinned.contains(room_id, &event_id) {
            return;
        }
        self.active_timeline.jump_to_event(event_id);
    }

    pub(super) fn step_pinned(&mut self, event_id: &str, direction: PinDirection) {
        let Some(room_id) = self.active_timeline.room_id() else {
            return;
        };
        if let Some(target) = self.pinned.step(room_id, event_id, direction) {
            self.active_timeline.jump_to_event(target);
        }
    }

    async fn open_room(&mut self, room_id: RoomId, focus: TimelineFocus) {
        self.audio.abandon_lookup();
        self.selection.room = Some(room_id.clone());
        self.attachments.follow(self.selection.room.as_ref());
        self.submissions.offer(self.selection.room.as_ref());
        let generation = self.selection.next_generation();
        let meta = self
            .room_directory
            .selected_room_meta(&self.selection)
            .unwrap_or_default();
        self.emit_selected_room(EmittedRoom {
            id: room_id.clone(),
            meta,
            generation,
            live: focus.is_live(),
        })
        .await;
        self.refresh_upgrades();
        if let Some(stickers) = self.port(|a| &a.image_packs) {
            self.packs
                .select_room(stickers, room_id.clone(), generation);
        }
        let Some(timeline) = self.port(|a| &a.timeline) else {
            return;
        };
        self.active_timeline
            .select_room(timeline, room_id, generation, focus)
            .await;
        self.event_source.retarget(&self.active_timeline);
        self.readers.retarget(&self.active_timeline);
    }

    pub(super) async fn retry_timeline(&mut self) {
        let Some(room_id) = self.selection.room.clone() else {
            return;
        };
        self.select_room(room_id).await;
    }

    pub(super) async fn refresh_selected_room(&mut self) {
        let Some(room_id) = self.selection.room.clone() else {
            return;
        };
        let Some(meta) = self.room_directory.selected_room_meta(&self.selection) else {
            self.drop_selected_room().await;
            self.removals.open(self.room_directory.removal(&room_id));
            return;
        };
        let next = EmittedRoom {
            id: room_id,
            meta,
            generation: self.selection.generation,
            live: self.active_timeline.is_live(),
        };
        if self.last_selected_room.as_ref() == Some(&next) {
            return;
        }
        let became_encrypted = next.meta.encrypted
            && self
                .last_selected_room
                .as_ref()
                .is_some_and(|last| last.id == next.id && !last.meta.encrypted);
        if became_encrypted {
            self.follow_identity_alerts(next.id.clone());
        }
        self.emit_selected_room(next).await;
    }

    async fn drop_selected_room(&mut self) {
        self.room_info.close();
        self.user_info.close();
        self.event_source.close();
        self.readers.close();
        self.mentions.reset();
        self.audio.abandon_lookup();
        self.selection.room = None;
        self.submissions.offer(None);
        let generation = self.selection.next_generation();
        self.emit_selected_room(EmittedRoom {
            id: RoomId::new(String::new()),
            meta: RoomMeta::default(),
            generation,
            live: true,
        })
        .await;
        self.packs.clear_room();
        self.sync_selected_room(None);
        self.pinned.clear();
        self.identity_alerts.clear();
        self.typing.clear();
        self.follow_presence();
        self.follow_calls();
        self.refresh_upgrades();
        self.active_timeline.clear_room(generation).await;
    }
}
