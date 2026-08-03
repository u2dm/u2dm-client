mod active_timeline;
mod establish;
mod lifecycle;
mod media;
mod recover;
mod room_directory;
mod selection;
mod session;
mod space_order;
mod stickers;
mod task_group;
mod verification;

use std::sync::Arc;

use active_timeline::ActiveTimeline;
use establish::EstablishedSession;
use lifecycle::Lifecycle;
use media::MediaActions;
use recover::Recovery;
use room_directory::RoomDirectory;
use selection::Selection;
use session::{AuthOutcome, SessionController};
use stickers::Stickers;
use task_group::TaskGroup;
use tokio::sync::{mpsc, watch};
use verification::VerificationController;

use crate::commands::effects::Effect;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::sync::DirectoryUpdate;
use crate::commands::ui::{UiCommand, ViewportChanged};
use crate::commands::view::{AppViewState, LoginActivity, LoginStep, Toast};
use crate::domain::account::AccountScope;
use crate::domain::models::{ConnectionStatus, PackId, RoomId, RoomList, Space, TimelineFocus};
use crate::ports::browser::BrowserPort;
use crate::ports::matrix::{AuthPort, AuthenticatedSession, SessionPort};
use crate::ports::media::MediaFilePort;
use crate::ports::output::AppOutputPort;
use crate::ports::storage::StoragePort;

enum EndReason {
    UserLogout,
    Expired,
}

#[derive(PartialEq, Eq)]
struct EmittedRoom {
    id: RoomId,
    name: String,
    member_count: u64,
    generation: i32,
}

pub(super) fn show_toast(output: &dyn AppOutputPort, toast: Toast) {
    output.publish(Box::new(move |view| view.toast = toast));
}

async fn undo_superseded_login(established: EstablishedSession) {
    tracing::info!("authentication superseded, undoing the login");
    let report = established.roll_back().await;
    if !report.is_clean() {
        tracing::warn!("superseded login not fully undone: {}", report.summary());
    }
}

pub struct AppService {
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    dir_in_tx: mpsc::UnboundedSender<DirectoryUpdate>,
    output: Arc<dyn AppOutputPort>,
    background: TaskGroup,
    operations: TaskGroup,
    session: SessionController,
    room_directory: RoomDirectory,
    active_timeline: ActiveTimeline,
    verification: VerificationController,
    media: MediaActions,
    stickers: Stickers,
    selection: Selection,
    last_selected_room: Option<EmittedRoom>,
    lifecycle: Lifecycle,
    active: Option<AuthenticatedSession>,
    auth_rx: Option<mpsc::UnboundedReceiver<AuthOutcome>>,
    blocked_reason: Option<String>,
}

impl AppService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        auth: Arc<dyn AuthPort>,
        storage: Arc<dyn StoragePort>,
        media_files: Arc<dyn MediaFilePort>,
        browser: Arc<dyn BrowserPort>,
        cmd_tx: mpsc::UnboundedSender<UiCommand>,
        dir_in_tx: mpsc::UnboundedSender<DirectoryUpdate>,
        output: Arc<dyn AppOutputPort>,
    ) -> Self {
        let lifecycle = Lifecycle::new();
        let (auth_tx, auth_rx) = mpsc::unbounded_channel::<AuthOutcome>();
        Self {
            session: SessionController::new(
                auth,
                storage,
                browser,
                Arc::clone(&output),
                lifecycle.clone(),
                auth_tx,
            ),
            room_directory: RoomDirectory::new(Arc::clone(&output)),
            active_timeline: ActiveTimeline::new(cmd_tx.clone(), Arc::clone(&output)),
            verification: VerificationController::new(Arc::clone(&output)),
            media: MediaActions::new(media_files, Arc::clone(&output)),
            stickers: Stickers::new(Arc::clone(&output)),
            cmd_tx,
            dir_in_tx,
            output,
            background: TaskGroup::new("background"),
            operations: TaskGroup::new("operations"),
            selection: Selection::default(),
            last_selected_room: None,
            lifecycle,
            active: None,
            auth_rx: Some(auth_rx),
            blocked_reason: None,
        }
    }

    pub async fn run(
        &mut self,
        mut cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
        mut dir_in_rx: mpsc::UnboundedReceiver<DirectoryUpdate>,
        mut scroll_in_rx: watch::Receiver<ViewportChanged>,
    ) {
        let Some(mut auth_rx) = self.auth_rx.take() else {
            return;
        };
        if let Recovery::Blocked(reason) = self.session.recover_interrupted_logins().await {
            self.block_sign_in(reason);
        }
        let mut dir_done = false;
        let mut scroll_done = false;
        let mut auth_done = false;
        loop {
            tokio::select! {
                maybe_cmd = cmd_rx.recv() => {
                    let Some(cmd) = maybe_cmd else { break };
                    Self::log_command(&cmd);
                    if self.dispatch(cmd).await {
                        break;
                    }
                }
                maybe_outcome = auth_rx.recv(), if !auth_done => {
                    match maybe_outcome {
                        Some(outcome) => self.complete_auth(outcome).await,
                        None => auth_done = true,
                    }
                }
                maybe_dir = dir_in_rx.recv(), if !dir_done => {
                    match maybe_dir {
                        Some(DirectoryUpdate::Rooms(rooms)) => {
                            self.handle_rooms_updated(rooms).await;
                        }
                        Some(DirectoryUpdate::Spaces(spaces)) => {
                            self.handle_spaces_updated(spaces);
                        }
                        None => dir_done = true,
                    }
                }
                changed = scroll_in_rx.changed(), if !scroll_done => {
                    if changed.is_err() {
                        scroll_done = true;
                    } else {
                        let viewport = scroll_in_rx.borrow_and_update().clone();
                        self.active_timeline.scroll_position_changed(
                            &viewport.room_id,
                            viewport.generation,
                            viewport.at_bottom,
                        );
                    }
                }
            }
        }
    }

    fn block_sign_in(&mut self, reason: String) {
        tracing::error!("signing in is blocked until the interrupted login is resolved: {reason}");
        self.lifecycle.block();
        self.blocked_reason = Some(reason);
        self.report_blocked();
    }

    fn report_blocked(&self) {
        let Some(reason) = self.blocked_reason.clone() else {
            return;
        };
        self.output.publish(Box::new(move |view| {
            view.lifecycle.step = LoginStep::Homeserver;
            view.lifecycle.activity = LoginActivity::Idle;
            view.lifecycle.messages = vec![UserMessage::about(
                UserMessageKind::InterruptedLoginUnresolved,
                &reason,
            )];
        }));
    }

    #[allow(clippy::too_many_lines)]
    async fn dispatch(&mut self, cmd: UiCommand) -> bool {
        let phase = self.lifecycle.phase();
        if !lifecycle::command_allowed(phase, &cmd) {
            tracing::debug!(?phase, command = %cmd, "rejecting command illegal in current phase");
            if phase == lifecycle::AppPhase::Blocked {
                self.report_blocked();
            }
            return false;
        }
        match cmd {
            UiCommand::RestoreSession => {
                self.session.spawn_restore_session(&mut self.operations);
            }
            UiCommand::CheckServer(homeserver) => {
                let attempt = self.lifecycle.begin_auth();
                self.session
                    .spawn_check_server(&mut self.operations, homeserver, attempt);
            }
            UiCommand::LoginPassword(creds) => {
                let attempt = self.lifecycle.begin_auth();
                self.session
                    .spawn_login_password(&mut self.operations, creds, attempt);
            }
            UiCommand::LoginOAuth => {
                let attempt = self.lifecycle.begin_auth();
                self.session
                    .spawn_login_oauth(&mut self.operations, attempt);
            }
            UiCommand::CancelOAuth => {
                if self.lifecycle.cancel_auth() {
                    self.session.cancel_oauth();
                }
            }
            UiCommand::BackToHomeserver => {
                self.session.back_to_homeserver();
            }
            UiCommand::FetchRooms => {
                self.handle_fetch_rooms().await;
            }
            UiCommand::SelectSpace(space) => {
                self.handle_select_space(space);
            }
            UiCommand::SelectSubspace(subspace) => {
                self.handle_select_subspace(subspace);
            }
            UiCommand::MoveSpace { from, to } => {
                self.move_space(from, to);
            }
            UiCommand::SpaceOrderWriteFailed { op, spaces, error } => {
                self.revert_space_orders(op, &spaces, &error);
            }
            UiCommand::SelectRoom(room_id) => {
                self.select_room(room_id).await;
            }
            UiCommand::RetryTimeline => {
                self.retry_timeline().await;
            }
            UiCommand::SendMessage {
                room_id,
                body,
                reply_to,
            } => {
                self.send_message(room_id, body, reply_to);
            }
            UiCommand::SendSticker {
                room_id,
                pack,
                shortcode,
                reply_to,
            } => {
                self.send_sticker(room_id, pack, shortcode, reply_to);
            }
            UiCommand::PaginateBackwards {
                room_id,
                generation,
            } => {
                self.active_timeline
                    .paginate_backwards(&room_id, generation);
            }
            UiCommand::PaginateForwards {
                room_id,
                generation,
            } => {
                self.active_timeline.paginate_forwards(&room_id, generation);
            }
            UiCommand::TimelinePaginationCompleted {
                room_id,
                generation,
                direction,
                outcome,
            } => {
                self.active_timeline
                    .complete_pagination(&room_id, generation, direction, outcome);
            }
            UiCommand::JumpToLatest {
                room_id,
                generation,
            } => {
                if self.active_timeline.is_live() {
                    self.active_timeline.jump_to_latest(&room_id, generation);
                } else if self.active_timeline.is_current(&room_id, generation) {
                    self.open_room(room_id, TimelineFocus::Live).await;
                }
            }
            UiCommand::JumpToEvent { event_id } => {
                self.active_timeline.jump_to_event(event_id);
            }
            UiCommand::RefocusTimeline {
                room_id,
                generation,
                focus,
            } => {
                if self.active_timeline.is_current(&room_id, generation) {
                    self.open_room(room_id, focus).await;
                }
            }
            UiCommand::OpenMedia { event_id } => {
                self.open_media(event_id);
            }
            UiCommand::SaveFile { event_id, filename } => {
                self.save_file(event_id, filename);
            }
            UiCommand::DismissToast => {
                show_toast(self.output.as_ref(), Toast::None);
            }
            UiCommand::AcceptVerification => {
                self.accept_verification();
            }
            UiCommand::RejectVerification => {
                self.reject_verification();
            }
            UiCommand::ConfirmVerification => {
                self.confirm_verification();
            }
            UiCommand::DismissVerification => {
                self.dismiss_verification();
            }
            UiCommand::SessionExpired => {
                self.end_session(EndReason::Expired).await;
            }
            UiCommand::Logout => {
                self.end_session(EndReason::UserLogout).await;
            }
            UiCommand::Quit => {
                self.handle_quit().await;
                return true;
            }
        }
        false
    }

    fn port<P: ?Sized>(
        &self,
        pick: impl FnOnce(&AuthenticatedSession) -> &Arc<P>,
    ) -> Option<Arc<P>> {
        self.active.as_ref().map(|a| Arc::clone(pick(a)))
    }

    fn set_selected_space(&self, id: String) {
        self.output
            .publish(Box::new(move |view| view.directory.space_id = id));
    }

    fn set_selected_subspace(&self, id: String) {
        self.output
            .publish(Box::new(move |view| view.directory.subspace_id = id));
    }

    fn set_connection(&self, status: ConnectionStatus) {
        self.output
            .publish(Box::new(move |view| view.connection = status));
    }

    fn emit_login_idle(&self) {
        self.output.publish(Box::new(|view| {
            view.lifecycle.activity = LoginActivity::Idle;
        }));
    }

    fn emit_login_success(&self, user_id: String) {
        self.output.publish(Box::new(move |view| {
            view.lifecycle.user_id = user_id;
            view.lifecycle.step = LoginStep::LoggedIn;
            view.lifecycle.activity = LoginActivity::Idle;
        }));
    }

    async fn emit_selected_room(
        &mut self,
        id: RoomId,
        name: String,
        member_count: u64,
        generation: i32,
    ) {
        self.last_selected_room = Some(EmittedRoom {
            id: id.clone(),
            name: name.clone(),
            member_count,
            generation,
        });
        self.output
            .emit(Effect::SelectedRoom {
                id,
                name,
                member_count,
                generation,
            })
            .await;
    }

    fn move_space(&mut self, from: usize, to: usize) {
        let Some(space_order) = self.port(|a| &a.space_order) else {
            return;
        };
        if let Some(write) = self.room_directory.move_space(from, to) {
            RoomDirectory::spawn_order_write(
                &mut self.operations,
                space_order,
                write,
                self.cmd_tx.clone(),
            );
        }
    }

    fn revert_space_orders(&mut self, op: u64, spaces: &[String], error: &str) {
        if !self.room_directory.rollback_space_orders(op, spaces) {
            return;
        }
        tracing::warn!(op, "reverting optimistic space order: {error}");
        show_toast(
            self.output.as_ref(),
            Toast::Error(UserMessage::new(UserMessageKind::SpaceOrderSaveFailed)),
        );
    }

    fn log_command(cmd: &UiCommand) {
        tracing::info!(command = %cmd, "handling command");
    }

    async fn handle_rooms_updated(&mut self, rooms: RoomList) {
        if self.active.is_none() {
            return;
        }
        if self.room_directory.store_rooms(rooms) {
            self.refresh_selected_room().await;
            self.room_directory.emit_directory(&self.selection);
        }
    }

    fn handle_spaces_updated(&mut self, spaces: Arc<[Space]>) {
        if self.active.is_none() {
            return;
        }
        if self.room_directory.store_spaces(spaces) {
            let outcome = self.room_directory.reconcile(&mut self.selection);
            if outcome.space_dropped {
                self.set_selected_space(String::new());
                self.set_selected_subspace(String::new());
            } else if outcome.subspace_dropped {
                self.set_selected_subspace(String::new());
            }
            self.room_directory.emit_directory(&self.selection);
        }
    }

    fn handle_select_space(&mut self, space: Option<RoomId>) {
        self.selection.set_space(space);
        self.set_selected_space(self.selection.space_id_str());
        self.set_selected_subspace(self.selection.subspace_id_str());
        self.room_directory.emit_subspaces(&self.selection);
        self.room_directory.emit_rooms(&self.selection);
    }

    fn handle_select_subspace(&mut self, subspace: Option<RoomId>) {
        self.selection.set_subspace(subspace);
        self.set_selected_subspace(self.selection.subspace_id_str());
        self.room_directory.emit_rooms(&self.selection);
    }

    async fn complete_auth(&mut self, outcome: AuthOutcome) {
        let Some(capability) = self.settle_auth_outcome(outcome).await else {
            return;
        };
        let user_id = capability.session.user_id.clone();
        tracing::info!(%user_id, "authenticated");
        self.active = Some(capability);
        self.emit_login_success(user_id);
        if let Err(e) = self.cmd_tx.send(UiCommand::FetchRooms) {
            tracing::warn!("failed to trigger room fetch: {e}");
        }
    }

    async fn settle_auth_outcome(&mut self, outcome: AuthOutcome) -> Option<AuthenticatedSession> {
        match outcome {
            AuthOutcome::Login {
                attempt,
                established,
            } => {
                if self.lifecycle.promote_to_syncing(attempt).is_none() {
                    undo_superseded_login(established).await;
                    if self.lifecycle.is_logged_out() {
                        self.emit_login_idle();
                    }
                    return None;
                }
                Some(established.commit().await)
            }
            AuthOutcome::Restore(session) => {
                if self.lifecycle.restore_succeeded().is_none() {
                    tracing::info!("restore superseded, dropping session");
                    return None;
                }
                Some(session)
            }
        }
    }

    fn send_message(&mut self, room_id: RoomId, body: String, reply_to: Option<String>) {
        let Some(timeline) = self.port(|a| &a.timeline) else {
            return;
        };
        self.active_timeline
            .spawn_send(&mut self.operations, timeline, room_id, body, reply_to);
    }

    fn send_sticker(
        &mut self,
        room_id: RoomId,
        pack: PackId,
        shortcode: String,
        reply_to: Option<String>,
    ) {
        if let Some(stickers) = self.port(|a| &a.stickers) {
            self.stickers.send(
                &mut self.operations,
                stickers,
                room_id,
                pack,
                shortcode,
                reply_to,
            );
        }
    }

    fn open_media(&mut self, event_id: String) {
        if let Some(media) = self.port(|a| &a.media) {
            self.media.open_media(media, event_id);
        }
    }

    fn save_file(&mut self, event_id: String, filename: String) {
        if let Some(media) = self.port(|a| &a.media) {
            self.media.save_file(media, event_id, filename);
        }
    }

    fn accept_verification(&mut self) {
        if let Some(verification) = self.port(|a| &a.verification) {
            self.verification
                .spawn_accept(&mut self.operations, verification);
        }
    }

    fn reject_verification(&mut self) {
        if let Some(verification) = self.port(|a| &a.verification) {
            self.verification
                .spawn_reject(&mut self.operations, verification);
        }
    }

    fn confirm_verification(&mut self) {
        if let Some(verification) = self.port(|a| &a.verification) {
            self.verification
                .spawn_confirm(&mut self.operations, verification);
        }
    }

    fn dismiss_verification(&mut self) {
        let verification = self.port(|a| &a.verification);
        self.verification
            .spawn_dismiss(&mut self.operations, verification);
    }

    async fn select_room(&mut self, room_id: RoomId) {
        self.open_room(room_id, TimelineFocus::Live).await;
    }

    async fn open_room(&mut self, room_id: RoomId, focus: TimelineFocus) {
        self.selection.room = Some(room_id.clone());
        let generation = self.selection.next_generation();
        let (name, member_count) = self
            .room_directory
            .selected_room_meta(&self.selection)
            .map_or_else(|| (String::new(), 0), |m| (m.name, m.member_count));
        self.emit_selected_room(room_id.clone(), name, member_count, generation)
            .await;
        if let Some(stickers) = self.port(|a| &a.stickers) {
            self.stickers
                .select_room(stickers, room_id.clone(), generation);
        }
        let Some(timeline) = self.port(|a| &a.timeline) else {
            return;
        };
        self.active_timeline
            .select_room(timeline, room_id, generation, focus)
            .await;
    }

    async fn retry_timeline(&mut self) {
        let Some(room_id) = self.selection.room.clone() else {
            return;
        };
        self.select_room(room_id).await;
    }

    async fn refresh_selected_room(&mut self) {
        let Some(room_id) = self.selection.room.clone() else {
            return;
        };
        let Some(meta) = self.room_directory.selected_room_meta(&self.selection) else {
            self.drop_selected_room().await;
            return;
        };
        let next = EmittedRoom {
            id: room_id,
            name: meta.name,
            member_count: meta.member_count,
            generation: self.selection.generation,
        };
        if self.last_selected_room.as_ref() == Some(&next) {
            return;
        }
        self.emit_selected_room(next.id, next.name, next.member_count, next.generation)
            .await;
    }

    async fn drop_selected_room(&mut self) {
        self.selection.room = None;
        let generation = self.selection.next_generation();
        self.emit_selected_room(RoomId::new(String::new()), String::new(), 0, generation)
            .await;
        self.stickers.clear_room();
        self.active_timeline.clear_room(generation).await;
    }

    async fn handle_fetch_rooms(&mut self) {
        let Some((sync, verification, lifecycle_port)) = self.active.as_ref().map(|a| {
            (
                Arc::clone(&a.sync),
                Arc::clone(&a.verification),
                Arc::clone(&a.lifecycle),
            )
        }) else {
            tracing::debug!("fetch rooms without an authenticated session, ignoring");
            return;
        };
        self.room_directory.connect();
        self.output.publish(Box::new(|view| {
            view.lifecycle.activity = LoginActivity::Syncing;
        }));
        self.background.restart().await;
        self.session
            .spawn_session_persister(&mut self.background, Arc::clone(&lifecycle_port));
        self.verification
            .spawn_forwarder(&mut self.background, verification);
        self.set_connection(ConnectionStatus::Connecting);
        RoomDirectory::spawn_sync_pipeline(
            &mut self.background,
            sync,
            Arc::clone(&self.output),
            self.cmd_tx.clone(),
            self.dir_in_tx.clone(),
        );
        self.session
            .spawn_user_avatar_fetch(&mut self.background, lifecycle_port);
    }

    fn ending_session(&self) -> Option<(AccountScope, Arc<dyn SessionPort>)> {
        self.active.as_ref().map(|a| {
            (
                AccountScope::from_session(&a.session),
                Arc::clone(&a.lifecycle),
            )
        })
    }

    async fn shutdown_all_tasks(&mut self) {
        tokio::join!(
            self.background.shutdown(),
            self.active_timeline.shutdown(),
            self.operations.restart(),
            self.media.cancel_and_drain(),
            self.stickers.restart(),
        );
    }

    async fn end_session(&mut self, reason: EndReason) {
        let Some(session) = self.lifecycle.begin_logout() else {
            return;
        };
        if matches!(reason, EndReason::Expired) {
            tracing::info!("session expired, clearing local state");
        }
        let ending = self.ending_session();
        self.output.replace(AppViewState::logged_out());
        self.output.emit(Effect::LoggedOut).await;
        self.shutdown_all_tasks().await;
        self.media.clear_session().await;
        self.room_directory.reset();
        self.selection = Selection::default();
        self.last_selected_room = None;
        self.active = None;
        match ending {
            Some((account, port)) => match reason {
                EndReason::UserLogout => {
                    self.session
                        .spawn_logout(&mut self.operations, session, account, port);
                }
                EndReason::Expired => {
                    self.session
                        .spawn_expire_session(&mut self.operations, session, account, port);
                }
            },
            None => {
                self.lifecycle.finish_logout(session);
            }
        }
    }

    async fn handle_quit(&mut self) {
        tokio::join!(
            self.background.shutdown(),
            self.active_timeline.shutdown(),
            self.operations.shutdown(),
            self.media.drain(),
            self.stickers.shutdown(),
        );
    }
}
