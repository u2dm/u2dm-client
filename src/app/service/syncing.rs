use std::sync::Arc;

use super::AppService;
use crate::app::room_directory::RoomDirectory;
use crate::commands::ui::{ComposerActivity, TimelineVisibility};
use crate::commands::view::LoginActivity;
use crate::domain::invite::InviteList;
use crate::domain::room::{RoomId, RoomList, Space};
use crate::domain::sync::ConnectionStatus;

impl AppService {
    fn set_connection(&self, status: ConnectionStatus) {
        self.output
            .publish(Box::new(move |view| view.connection = status));
    }

    pub(super) async fn handle_rooms_updated(&mut self, rooms: RoomList) {
        if self.held_session.running().is_none() {
            return;
        }
        if self.room_directory.store_rooms(rooms) {
            self.refresh_selected_room().await;
            self.room_directory.emit_directory(&self.selection);
            self.follow_removals();
            self.follow_space_leave();
            self.notifications.directory_changed(&self.room_directory);
            self.calls.directory_changed(&self.room_directory);
            self.refresh_space_index();
            self.follow_room_info();
            self.follow_presence();
            if let Some(ready) = self.user_info.follow(self.room_directory.rooms()) {
                self.direct_chat_ready(ready).await;
            }
            self.follow_invites().await;
            self.follow_upgrades().await;
            self.follow_previews().await;
            self.follow_creations().await;
            self.follow_invite_people();
            self.release_held_link().await;
        }
    }

    pub(super) async fn handle_invites_updated(&mut self, invites: InviteList) {
        if self.held_session.running().is_none() {
            return;
        }
        if self.room_directory.store_invites(invites) {
            self.room_directory.emit_directory(&self.selection);
            self.notifications.directory_changed(&self.room_directory);
            self.follow_removals();
            self.follow_invites().await;
            self.follow_previews().await;
        }
    }

    pub(super) async fn handle_spaces_updated(&mut self, spaces: Arc<[Space]>) {
        if self.held_session.running().is_none() {
            return;
        }
        if self.room_directory.store_spaces(spaces) {
            let outcome = self.room_directory.reconcile(&mut self.selection);
            if outcome.space_dropped || outcome.subspace_dropped {
                self.publish_selection();
            }
            self.room_directory.emit_directory(&self.selection);
            self.notifications.directory_changed(&self.room_directory);
            self.follow_space_index();
            self.room_info.follow_spaces(self.room_directory.spaces());
            self.follow_removals();
            self.follow_space_leave();
            self.follow_invites().await;
            self.follow_previews().await;
            self.follow_creations().await;
            self.follow_invite_people();
        }
    }

    pub(super) fn timeline_visibility_changed(&mut self, visibility: TimelineVisibility) {
        self.active_timeline.visibility_changed(visibility);
        self.notifications
            .on_screen(self.active_timeline.on_screen());
    }

    pub(super) fn sync_selected_room(&self, room_id: Option<&RoomId>) {
        if let Some(sync) = self.port(|a| &a.sync) {
            sync.set_selected_room(room_id);
        }
    }

    pub(super) fn composer_changed(&mut self, activity: &ComposerActivity) {
        let port = self.port(|a| &a.typing);
        self.typing
            .composer_changed(port, self.selection.room.as_ref(), activity);
    }

    pub(super) async fn start_syncing(&mut self, session: u64) {
        let Some((sync, verification, identity, encryption, push, lifecycle_port)) =
            self.held_session.running().map(|a| {
                (
                    Arc::clone(&a.sync),
                    Arc::clone(&a.verification),
                    Arc::clone(&a.identity),
                    Arc::clone(&a.encryption),
                    Arc::clone(&a.push),
                    Arc::clone(&a.lifecycle),
                )
            })
        else {
            tracing::debug!("no authenticated session to sync, ignoring");
            return;
        };
        self.room_directory.connect();
        self.output.publish(Box::new(|view| {
            view.lifecycle.activity = LoginActivity::Syncing;
        }));
        self.background.restart().await;
        self.session.spawn_session_persister(
            &mut self.background,
            Arc::clone(&lifecycle_port),
            session,
        );
        self.verification
            .spawn_listener(&mut self.background, verification, session);
        self.identity_alerts
            .spawn_own_listener(&mut self.background, identity, session);
        self.encryption
            .begin_session(&mut self.background, &encryption, session);
        self.notifications
            .spawn_listener(&mut self.background, push, session);
        self.set_connection(ConnectionStatus::Connecting);
        RoomDirectory::spawn_sync_pipeline(
            &mut self.background,
            sync,
            Arc::clone(&self.output),
            self.events.clone(),
            self.dir_in_tx.clone(),
        );
        self.session
            .spawn_user_avatar_fetch(&mut self.background, lifecycle_port, session);
        self.begin_presence(session);
        self.begin_calls(session);
    }
}
