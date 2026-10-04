mod active_timeline;
mod attachments;
mod audio;
mod conclude;
mod credentials;
mod establish;
mod event;
mod event_source;
pub mod input;
mod lifecycle;
mod link_requests;
mod media;
mod message_actions;
mod pinned;
mod polls;
mod recover;
mod room_directory;
mod room_info;
mod selection;
mod send_lanes;
mod session;
mod space_index;
mod space_order;
mod stickers;
mod submissions;
mod task_group;
mod verification;
mod video;

use std::sync::Arc;

use active_timeline::ActiveTimeline;
use attachments::Attachments;
use audio::AudioController;
use establish::{EstablishedSession, Rollback};
use event::{
    AppEvent, EndReason, MessageActionEvent, RoomActionEvent, SessionEvent, TimelineEvent,
};
use event_source::EventSourceViewer;
use input::{CommandSender, EventSender, Inbox, Input};
use lifecycle::{Lifecycle, Settled};
use media::MediaActions;
use message_actions::MessageActions;
use pinned::PinnedMessages;
use recover::Recovery;
use room_directory::{RoomDirectory, RoomMeta};
use room_info::RoomInfo;
use selection::Selection;
use send_lanes::SendLanes;
use session::{SessionController, SuspendedSession};
use space_index::{ChildTarget, JoinOutcome, ListedSpace, PageOutcome, SpaceIndex};
use stickers::Stickers;
use submissions::Submissions;
use task_group::TaskGroup;
use tokio::sync::{mpsc, watch};
use verification::VerificationController;
use video::VideoController;

use crate::commands::effects::Effect;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::sync::DirectoryUpdate;
use crate::commands::ui::{MessageDraft, TimelineVisibility, UiCommand, ViewportChanged};
use crate::commands::view::{AppViewState, LoginActivity, LoginStep, Toast};
use crate::domain::account::AccountScope;
use crate::domain::auth::ServerInfo;
use crate::domain::media::AttachmentPick;
use crate::domain::message::{MessageEdit, PinChange};
use crate::domain::poll::PollDraft;
use crate::domain::room::{NotifyMode, RoomId, RoomList, Space};
use crate::domain::sticker::PackId;
use crate::domain::sync::ConnectionStatus;
use crate::domain::timeline::{AudioTrack, FailedSend, TimelineFocus};
use crate::ports::browser::BrowserPort;
use crate::ports::matrix::{AuthPort, AuthenticatedSession, CleanupReport, MediaPort, SessionPort};
use crate::ports::media::MediaFilePort;
use crate::ports::output::AppOutputPort;
use crate::ports::storage::StoragePort;

#[derive(PartialEq, Eq)]
struct EmittedRoom {
    id: RoomId,
    meta: RoomMeta,
    generation: i32,
    live: bool,
}

pub(super) fn show_toast(output: &dyn AppOutputPort, toast: Toast) {
    output.publish(Box::new(move |view| view.toast = toast));
}

enum Reauth {
    Password(String),
    Browser,
}

enum HeldSession {
    None,
    Running(Box<AuthenticatedSession>),
    Suspended(Box<SuspendedSession>),
}

enum Ending {
    Live(AccountScope, Arc<dyn SessionPort>),
    Detached(AccountScope, Arc<dyn SessionPort>),
}

impl HeldSession {
    fn running(&self) -> Option<&AuthenticatedSession> {
        match self {
            Self::Running(capability) => Some(capability),
            Self::None | Self::Suspended(_) => None,
        }
    }

    fn ending(&self) -> Option<Ending> {
        match self {
            Self::None => None,
            Self::Running(capability) => Some(Ending::Live(
                AccountScope::from_session(&capability.session),
                Arc::clone(&capability.lifecycle),
            )),
            Self::Suspended(suspended) => Some(Ending::Detached(
                suspended.account.clone(),
                Arc::clone(&suspended.lifecycle),
            )),
        }
    }

    fn suspended(&self) -> Option<&SuspendedSession> {
        match self {
            Self::Suspended(suspended) => Some(suspended),
            Self::None | Self::Running(_) => None,
        }
    }
}

async fn undo_superseded_login(established: EstablishedSession) -> Option<UserMessage> {
    tracing::info!("authentication superseded, undoing the login");
    match established.roll_back().await {
        Rollback::Complete(report) => {
            if !report.is_clean() {
                tracing::warn!("superseded login not fully undone: {}", report.summary());
            }
            None
        }
        Rollback::Unresolved(message) => Some(message),
    }
}

pub struct AppService {
    events: EventSender,
    dir_in_tx: mpsc::UnboundedSender<DirectoryUpdate>,
    output: Arc<dyn AppOutputPort>,
    background: TaskGroup,
    operations: TaskGroup,
    send_lanes: SendLanes,
    session: SessionController,
    room_directory: RoomDirectory,
    active_timeline: ActiveTimeline,
    pinned: PinnedMessages,
    verification: VerificationController,
    media: MediaActions,
    audio: AudioController,
    video: VideoController,
    stickers: Stickers,
    space_index: SpaceIndex,
    room_info: RoomInfo,
    message_actions: MessageActions,
    event_source: EventSourceViewer,
    attachments: Attachments,
    submissions: Submissions,
    selection: Selection,
    last_selected_room: Option<EmittedRoom>,
    lifecycle: Lifecycle,
    held_session: HeldSession,
    blocked: Option<UserMessage>,
}

impl AppService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        auth: Arc<dyn AuthPort>,
        storage: Arc<dyn StoragePort>,
        media_files: Arc<dyn MediaFilePort>,
        browser: Arc<dyn BrowserPort>,
        commands: &CommandSender,
        dir_in_tx: mpsc::UnboundedSender<DirectoryUpdate>,
        output: Arc<dyn AppOutputPort>,
    ) -> Self {
        let events = commands.events();
        Self {
            session: SessionController::new(
                auth,
                storage,
                browser,
                Arc::clone(&output),
                events.clone(),
            ),
            room_directory: RoomDirectory::new(Arc::clone(&output)),
            active_timeline: ActiveTimeline::new(events.clone(), Arc::clone(&output)),
            pinned: PinnedMessages::new(Arc::clone(&output), events.clone()),
            verification: VerificationController::new(Arc::clone(&output), events.clone()),
            media: MediaActions::new(Arc::clone(&media_files), Arc::clone(&output)),
            audio: AudioController::new(Arc::clone(&output), events.clone()),
            video: VideoController::new(Arc::clone(&output), events.clone()),
            stickers: Stickers::new(Arc::clone(&output)),
            space_index: SpaceIndex::new(Arc::clone(&output), events.clone()),
            room_info: RoomInfo::new(Arc::clone(&output), events.clone()),
            message_actions: MessageActions::new(Arc::clone(&output), events.clone()),
            event_source: EventSourceViewer::new(Arc::clone(&output)),
            attachments: Attachments::new(media_files, Arc::clone(&output), events.clone()),
            submissions: Submissions::new(Arc::clone(&output), events.clone()),
            events,
            dir_in_tx,
            output,
            background: TaskGroup::new("background"),
            operations: TaskGroup::new("operations"),
            send_lanes: SendLanes::new("sends"),
            selection: Selection::default(),
            last_selected_room: None,
            lifecycle: Lifecycle::new(),
            held_session: HeldSession::None,
            blocked: None,
        }
    }

    pub async fn run(
        &mut self,
        mut inbox: Inbox,
        mut dir_in_rx: mpsc::UnboundedReceiver<DirectoryUpdate>,
        mut scroll_in_rx: watch::Receiver<ViewportChanged>,
        mut visibility_in_rx: watch::Receiver<TimelineVisibility>,
    ) {
        if let Recovery::Blocked(message) = self.session.recover_interrupted_logins().await {
            self.block_sign_in(message);
        }
        let mut dir_done = false;
        let mut scroll_done = false;
        let mut visibility_done = false;
        loop {
            tokio::select! {
                maybe_input = inbox.recv() => {
                    let Some(input) = maybe_input else { break };
                    if self.handle_input(input).await {
                        break;
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
                            viewport.unread_below,
                        );
                    }
                }
                changed = visibility_in_rx.changed(), if !visibility_done => {
                    if changed.is_err() {
                        visibility_done = true;
                    } else {
                        let visibility = *visibility_in_rx.borrow_and_update();
                        self.active_timeline.visibility_changed(visibility);
                    }
                }
            }
        }
    }

    async fn handle_input(&mut self, input: Input) -> bool {
        match input {
            Input::Ui(cmd) => {
                tracing::info!(command = %cmd, "handling command");
                self.dispatch(cmd).await
            }
            Input::Internal(event) => {
                tracing::debug!(event = event.label(), "handling app event");
                self.handle_event(event).await;
                false
            }
        }
    }

    fn block_sign_in(&mut self, message: UserMessage) {
        tracing::error!(
            "signing in is blocked until this is resolved: {detail}",
            detail = message.detail
        );
        self.lifecycle.block();
        self.blocked = Some(message);
        self.report_blocked();
    }

    fn report_blocked(&self) {
        let Some(message) = self.blocked.clone() else {
            return;
        };
        self.output.publish(Box::new(move |view| {
            view.lifecycle.step = LoginStep::Homeserver;
            view.lifecycle.activity = LoginActivity::Idle;
            view.lifecycle.messages = vec![message];
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
                self.lifecycle.begin_restore();
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
                self.cancel_oauth();
            }
            UiCommand::BackToHomeserver => {
                self.session.back_to_homeserver();
            }
            UiCommand::ReauthPassword(password) => {
                self.reauthenticate(Reauth::Password(password));
            }
            UiCommand::ReauthOAuth => {
                self.reauthenticate(Reauth::Browser);
            }
            UiCommand::SelectSpace(space) => {
                self.handle_select_space(space);
            }
            UiCommand::SelectDirect => {
                self.handle_select_direct();
            }
            UiCommand::SelectSubspace(subspace) => {
                self.handle_select_subspace(subspace);
            }
            UiCommand::MoveSpace { from, to } => {
                self.move_space(from, to);
            }
            UiCommand::SelectRoom(room_id) => {
                self.select_room(room_id).await;
            }
            UiCommand::OpenSpaceIndex => {
                self.open_space_index();
            }
            UiCommand::CloseSpaceIndex => {
                self.space_index.close();
            }
            UiCommand::PageSpaceIndex => {
                self.page_space_index();
            }
            UiCommand::RetrySpaceIndex => {
                self.retry_space_index();
            }
            UiCommand::JoinSpaceChild(room_id) => {
                self.join_space_child(room_id);
            }
            UiCommand::OpenSpaceChild(room_id) => {
                self.open_space_child(room_id).await;
            }
            UiCommand::OpenRoomInfo(room_id) => {
                self.open_room_info(&room_id);
            }
            UiCommand::CloseRoomInfo => {
                self.room_info.close();
            }
            UiCommand::PageRoomMembers => {
                self.page_room_members();
            }
            UiCommand::RetryRoomMembers => {
                self.retry_room_members();
            }
            UiCommand::FilterRoomMembers(query) => {
                self.filter_room_members(query);
            }
            UiCommand::SetRoomNotify { room_id, mode } => {
                self.set_room_notify(&room_id, mode);
            }
            UiCommand::LeaveRoom(room_id) => {
                self.leave_room(&room_id);
            }
            UiCommand::OpenRoomMenu(room_id) => {
                self.room_info.open_menu(self.room_directory.room(&room_id));
            }
            UiCommand::CloseRoomMenu => {
                self.room_info.close_menu();
            }
            UiCommand::MarkRoomRead(room_id) => {
                self.mark_room_read(&room_id);
            }
            UiCommand::CopyRoomLink(room_id) => {
                self.copy_room_link(&room_id);
            }
            UiCommand::RetryTimeline => {
                self.retry_timeline().await;
            }
            UiCommand::SendMessage { room_id, draft } => {
                self.send_message(room_id, draft).await;
            }
            UiCommand::EditMessage { room_id, edit } => {
                self.edit_message(room_id, edit);
            }
            UiCommand::DismissUnsent { submission } => {
                self.submissions
                    .dismiss(submission, self.selection.room.as_ref());
            }
            UiCommand::PickAttachment { room_id, pick } => {
                self.pick_attachment(room_id, pick);
            }
            UiCommand::SendAttachment {
                room_id,
                caption,
                as_document,
                reply_to,
            } => {
                self.send_attachment(room_id, caption, as_document, reply_to)
                    .await;
            }
            UiCommand::CancelAttachment => {
                self.attachments.cancel();
            }
            UiCommand::SendSticker {
                room_id,
                pack,
                shortcode,
                reply_to,
            } => {
                self.send_sticker(room_id, pack, shortcode, reply_to).await;
            }
            UiCommand::SendPoll { room_id, draft } => {
                self.send_poll(room_id, draft).await;
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
            UiCommand::JumpToLatest {
                room_id,
                generation,
            } => {
                self.jump_to_latest(room_id, generation).await;
            }
            UiCommand::JumpToEvent { event_id } => {
                self.active_timeline.jump_to_event(event_id);
            }
            UiCommand::OpenPinned { event_id } => {
                self.open_pinned(event_id);
            }
            UiCommand::CopyMessageLink { event_id } => {
                self.copy_message_link(event_id);
            }
            UiCommand::OpenEventSource { event_id } => {
                self.event_source.open(&self.active_timeline, event_id);
            }
            UiCommand::CloseEventSource => {
                self.event_source.close();
            }
            UiCommand::PinMessage { event_id } => {
                self.change_pin(event_id, PinChange::Pin);
            }
            UiCommand::UnpinMessage { event_id } => {
                self.change_pin(event_id, PinChange::Unpin);
            }
            UiCommand::DeleteMessage { event_id } => {
                self.delete_message(event_id);
            }
            UiCommand::RetrySend { local_id } => {
                self.resolve_failed_send(local_id, FailedSend::Retry);
            }
            UiCommand::DiscardSend { local_id } => {
                self.resolve_failed_send(local_id, FailedSend::Discard);
            }
            UiCommand::ToggleReaction { event_id, key } => {
                self.active_timeline.toggle_reaction(event_id, key);
            }
            UiCommand::VotePoll {
                event_id,
                answer_id,
            } => {
                self.active_timeline.vote_poll(event_id, answer_id);
            }
            UiCommand::EditPoll { event_id, draft } => {
                self.active_timeline.edit_poll(event_id, draft);
            }
            UiCommand::EndPoll { event_id } => {
                self.active_timeline.end_poll(event_id);
            }
            UiCommand::OpenMedia { event_id } => {
                self.open_media(event_id);
            }
            UiCommand::OpenVideo { event_id } => {
                self.open_video(event_id);
            }
            UiCommand::CloseVideo => {
                self.video.close();
            }
            UiCommand::PlayAudio { event_id } => {
                self.play_audio(event_id);
            }
            UiCommand::CloseAudio => {
                self.audio.close();
            }
            UiCommand::AudioEnded { request, end } => {
                self.audio.ended(&self.active_timeline, request, end);
            }
            UiCommand::OpenLink { url } => {
                self.session.spawn_open_link(&mut self.operations, url);
            }
            UiCommand::SaveFile { event_id, filename } => {
                self.save_file(event_id, filename);
            }
            UiCommand::DismissToast => {
                show_toast(self.output.as_ref(), Toast::None);
            }
            UiCommand::AcceptVerification => {
                self.accept_verification().await;
            }
            UiCommand::RejectVerification => {
                self.reject_verification().await;
            }
            UiCommand::ConfirmVerification => {
                self.confirm_verification().await;
            }
            UiCommand::DismissVerification => {
                self.dismiss_verification().await;
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

    fn cancel_oauth(&mut self) {
        if self.lifecycle.cancel_auth() {
            self.session.cancel_oauth();
        }
    }

    fn reauthenticate(&mut self, method: Reauth) {
        let Some(prior) = self
            .held_session
            .suspended()
            .map(|suspended| suspended.session.clone())
        else {
            return;
        };
        let Some(attempt) = self.lifecycle.begin_reauth() else {
            return;
        };
        match method {
            Reauth::Password(password) => {
                self.session
                    .spawn_reauth_password(&mut self.operations, prior, password, attempt);
            }
            Reauth::Browser => {
                self.session
                    .spawn_reauth_oauth(&mut self.operations, prior, attempt);
            }
        }
    }

    async fn jump_to_latest(&mut self, room_id: RoomId, generation: i32) {
        if self.active_timeline.is_live() {
            self.active_timeline.jump_to_latest(&room_id, generation);
        } else if self.active_timeline.is_current(&room_id, generation) {
            self.open_room(room_id, TimelineFocus::Latest).await;
        }
    }

    async fn refocus_timeline(&mut self, room_id: RoomId, generation: i32, focus: TimelineFocus) {
        if self.active_timeline.is_current(&room_id, generation) {
            self.open_room(room_id, focus).await;
        }
    }

    async fn return_to_live_for_send(&mut self, room_id: RoomId) {
        if !self.active_timeline.is_live() && self.active_timeline.is_active_room(&room_id) {
            self.open_room(room_id, TimelineFocus::Latest).await;
        }
    }

    fn port<P: ?Sized>(
        &self,
        pick: impl FnOnce(&AuthenticatedSession) -> &Arc<P>,
    ) -> Option<Arc<P>> {
        self.held_session.running().map(|a| Arc::clone(pick(a)))
    }

    fn publish_selection(&self) {
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

    fn set_connection(&self, status: ConnectionStatus) {
        self.output
            .publish(Box::new(move |view| view.connection = status));
    }

    fn emit_login_success(&self, user_id: String) {
        self.output.publish(Box::new(move |view| {
            view.lifecycle.user_id = user_id;
            view.lifecycle.step = LoginStep::LoggedIn;
            view.lifecycle.activity = LoginActivity::Idle;
        }));
    }

    async fn emit_selected_room(&mut self, room: EmittedRoom) {
        let effect = Effect::SelectedRoom {
            id: room.id.clone(),
            name: room.meta.name.clone(),
            member_count: room.meta.member_count,
            encrypted: room.meta.encrypted,
            polls: room.meta.polls,
            messages: room.meta.messages,
            generation: room.generation,
            live: room.live,
        };
        self.last_selected_room = Some(room);
        self.output.emit(effect).await;
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
                self.events.clone(),
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

    async fn handle_rooms_updated(&mut self, rooms: RoomList) {
        if self.held_session.running().is_none() {
            return;
        }
        if self.room_directory.store_rooms(rooms) {
            self.refresh_selected_room().await;
            self.room_directory.emit_directory(&self.selection);
            self.refresh_space_index();
            self.follow_room_info();
        }
    }

    fn handle_spaces_updated(&mut self, spaces: Arc<[Space]>) {
        if self.held_session.running().is_none() {
            return;
        }
        if self.room_directory.store_spaces(spaces) {
            let outcome = self.room_directory.reconcile(&mut self.selection);
            if outcome.space_dropped || outcome.subspace_dropped {
                self.publish_selection();
            }
            self.room_directory.emit_directory(&self.selection);
            self.follow_space_index();
        }
    }

    fn handle_select_space(&mut self, space: Option<RoomId>) {
        self.selection.set_space(space);
        self.show_selected_scope();
        self.follow_space_index();
    }

    fn handle_select_direct(&mut self) {
        self.selection.set_direct();
        self.show_selected_scope();
        self.follow_space_index();
    }

    fn show_selected_scope(&self) {
        self.publish_selection();
        self.room_directory.emit_subspaces(&self.selection);
        self.room_directory.emit_rooms(&self.selection);
    }

    fn handle_select_subspace(&mut self, subspace: Option<RoomId>) {
        self.selection.set_subspace(subspace);
        self.publish_selection();
        self.room_directory.emit_rooms(&self.selection);
        self.follow_space_index();
    }

    fn listed_space(&self) -> Option<ListedSpace> {
        let id = self.selection.listed_space()?.clone();
        let name = self
            .room_directory
            .space_name(&id)
            .unwrap_or_default()
            .to_owned();
        Some(ListedSpace { id, name })
    }

    fn open_space_index(&mut self) {
        if let Some(port) = self.port(|a| &a.space_index) {
            let target = self.listed_space();
            self.space_index.open(port, target);
        }
    }

    fn page_space_index(&mut self) {
        if let Some(port) = self.port(|a| &a.space_index) {
            self.space_index.page(port);
        }
    }

    fn retry_space_index(&mut self) {
        if let Some(port) = self.port(|a| &a.space_index) {
            self.space_index.retry(port);
        }
    }

    fn follow_space_index(&mut self) {
        let Some(port) = self.port(|a| &a.space_index) else {
            return;
        };
        let target = self.listed_space();
        let membership = self.room_directory.membership(&self.selection);
        self.space_index.follow(port, target, &membership);
    }

    fn refresh_space_index(&mut self) {
        if !self.space_index.is_tracking() {
            return;
        }
        let membership = self.room_directory.membership(&self.selection);
        self.space_index.membership_changed(&membership);
    }

    fn settle_space_index_page(&mut self, generation: u64, outcome: PageOutcome) {
        let Some(port) = self.port(|a| &a.space_index) else {
            return;
        };
        let membership = self.room_directory.membership(&self.selection);
        self.space_index
            .fetched(port, generation, outcome, &membership);
    }

    fn join_space_child(&mut self, room_id: RoomId) {
        let Some(port) = self.port(|a| &a.space_index) else {
            return;
        };
        let membership = self.room_directory.membership(&self.selection);
        self.space_index.join(port, room_id, &membership);
    }

    fn settle_space_child_join(&mut self, room_id: RoomId, name: &str, outcome: JoinOutcome) {
        let membership = self.room_directory.membership(&self.selection);
        self.space_index
            .join_settled(room_id, name, outcome, &membership);
    }

    async fn open_space_child(&mut self, room_id: RoomId) {
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

    fn open_room_info(&mut self, room_id: &RoomId) {
        if self.selection.room.as_ref() != Some(room_id) {
            tracing::debug!(%room_id, "ignoring room info for a room that is not selected");
            return;
        }
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info.open(port, self.room_directory.room(room_id));
        }
    }

    fn page_room_members(&mut self) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info.page(port);
        }
    }

    fn retry_room_members(&mut self) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info.retry(port);
        }
    }

    fn filter_room_members(&mut self, query: String) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info.filter(port, query);
        }
    }

    fn set_room_notify(&mut self, room_id: &RoomId, mode: NotifyMode) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info
                .set_notify(port, self.room_directory.room(room_id), mode);
        }
    }

    fn leave_room(&mut self, room_id: &RoomId) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info
                .leave(port, self.room_directory.room(room_id));
        }
    }

    fn mark_room_read(&mut self, room_id: &RoomId) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info
                .mark_read(port, self.room_directory.room(room_id));
        }
    }

    fn copy_room_link(&mut self, room_id: &RoomId) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info
                .copy_link(port, self.room_directory.room(room_id));
        }
    }

    fn follow_room_info(&mut self) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info.follow(port, self.room_directory.rooms());
        }
    }

    async fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Session(event) => self.handle_session_event(event).await,
            AppEvent::Timeline(event) => self.handle_timeline_event(event).await,
            AppEvent::SpaceOrderWriteFailed { op, spaces, error } => {
                self.revert_space_orders(op, &spaces, &error);
            }
            AppEvent::VerificationFlow(event) => self.verification.flow_advanced(event).await,
            AppEvent::VerificationActionFailed(failure) => {
                self.verification.action_failed(failure).await;
            }
            AppEvent::AttachmentPicked(picked) => {
                self.attachments
                    .adopt(*picked, self.selection.room.as_ref());
            }
            AppEvent::AttachmentSettled {
                submission,
                failure,
            } => {
                self.attachments.settle(submission, failure);
            }
            AppEvent::AudioFetched { request, outcome } => {
                self.audio.fetched(request, outcome);
            }
            AppEvent::VideoFetched { request, outcome } => {
                self.video.fetched(request, outcome);
            }
            AppEvent::SubmissionSettled {
                submission,
                enqueue,
            } => {
                self.submissions
                    .settled(submission, enqueue, self.selection.room.as_ref());
            }
            AppEvent::EditUnsaved { room_id, edit } => {
                self.hold_unsaved_edit(&room_id, edit);
            }
            AppEvent::PinnedChanged { watch, messages } => {
                self.pinned.changed(watch, messages);
            }
            AppEvent::SpaceIndexPaged {
                generation,
                outcome,
            } => {
                self.settle_space_index_page(generation, outcome);
            }
            AppEvent::SpaceIndexAvatarsReady { generation, ready } => {
                self.space_index.avatars_ready(generation, ready);
            }
            AppEvent::SpaceChildJoinSettled {
                room_id,
                name,
                outcome,
            } => {
                self.settle_space_child_join(room_id, &name, outcome);
            }
            AppEvent::RoomRosterFetched {
                generation,
                outcome,
            } => {
                if let Some(port) = self.port(|a| &a.room_info) {
                    self.room_info.roster_fetched(port, generation, outcome);
                }
            }
            AppEvent::RoomAboutFetched { generation, about } => {
                self.room_info.about_fetched(generation, about);
            }
            AppEvent::RoomInfoAvatarsReady { generation, ready } => {
                self.room_info.avatars_ready(generation, ready);
            }
            AppEvent::RoomAction(event) => self.handle_room_action(event),
            AppEvent::MessageAction(event) => self.handle_message_action(event),
        }
    }

    fn handle_room_action(&mut self, event: RoomActionEvent) {
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

    fn handle_message_action(&mut self, event: MessageActionEvent) {
        match event {
            MessageActionEvent::LinkResolved { request, link } => {
                self.message_actions.link_resolved(request, link);
            }
            MessageActionEvent::PinSettled {
                request,
                event_id,
                outcome,
            } => self.pinned.pin_settled(request, &event_id, outcome),
            MessageActionEvent::DeletionSettled { event_id, outcome } => {
                self.message_actions.deletion_settled(&event_id, outcome);
            }
        }
    }

    async fn handle_timeline_event(&mut self, event: TimelineEvent) {
        match event {
            TimelineEvent::Advanced {
                room_id,
                generation,
                advance,
            } => self
                .active_timeline
                .settle_read_position(&room_id, generation, advance),
            TimelineEvent::PaginationCompleted {
                room_id,
                generation,
                direction,
                outcome,
            } => self
                .active_timeline
                .complete_pagination(&room_id, generation, direction, outcome),
            TimelineEvent::Refocus {
                room_id,
                generation,
                focus,
            } => self.refocus_timeline(room_id, generation, focus).await,
            TimelineEvent::AudioLocated {
                room_id,
                generation,
                request,
                track,
            } => self.settle_audio_lookup(&room_id, generation, request, track),
            TimelineEvent::SourceLocated {
                room_id,
                generation,
                request,
                source,
            } => {
                if self.active_timeline.is_current(&room_id, generation) {
                    self.event_source.located(request, source);
                }
            }
        }
    }

    async fn handle_session_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::RestoreProgress(activity) => self.settle_restore_progress(activity),
            SessionEvent::RestoreFailed(message) => self.settle_restore_failure(message),
            SessionEvent::RestorePaused(message) => self.settle_restore_pause(message),
            SessionEvent::Restored(capability) => self.settle_restore(*capability).await,
            SessionEvent::ServerDiscovered { attempt, info } => {
                self.settle_discovery(attempt, *info);
            }
            SessionEvent::AuthActivity { attempt, activity } => {
                self.settle_auth_activity(attempt, activity);
            }
            SessionEvent::AuthRejected { attempt, message } => {
                self.settle_auth_rejection(attempt, message);
            }
            SessionEvent::AuthCancelled { attempt } => self.settle_auth_cancel(attempt),
            SessionEvent::LoggedIn {
                attempt,
                established,
            } => self.settle_login(attempt, *established).await,
            SessionEvent::LoginUnresolved(message) => self.block_sign_in(message),
            SessionEvent::ErasingLocalState { session } => self.settle_erasure_start(session),
            SessionEvent::LocalStateCleared {
                session,
                reason,
                report,
            } => self.settle_erasure(session, reason, &report),
            SessionEvent::TokensNotPersisted => show_toast(
                self.output.as_ref(),
                Toast::Error(UserMessage::new(UserMessageKind::SessionSaveFailed)),
            ),
            SessionEvent::UserAvatar(path) => {
                self.output
                    .publish(Box::new(move |view| view.lifecycle.avatar_path = path));
            }
            SessionEvent::Resumed {
                attempt,
                capability,
            } => self.settle_resume(attempt, *capability).await,
            SessionEvent::Suspended => self.suspend_session().await,
            SessionEvent::Expired => self.end_session(EndReason::Expired).await,
        }
    }

    fn settle_restore_progress(&self, activity: LoginActivity) {
        if self.lifecycle.is_restoring() {
            self.session.set_activity(activity);
        }
    }

    fn settle_restore_failure(&mut self, message: Option<UserMessage>) {
        if self.lifecycle.restore_failed() {
            self.session.show_login(message);
        } else {
            tracing::debug!("restore failure for a superseded restore, dropping");
        }
    }

    fn settle_restore_pause(&mut self, message: UserMessage) {
        if self.lifecycle.pause_restore() {
            self.session.show_restore_paused(message);
        } else {
            tracing::debug!("restore pause for a superseded restore, dropping");
        }
    }

    async fn settle_restore(&mut self, capability: AuthenticatedSession) {
        if self.lifecycle.restore_succeeded().is_none() {
            tracing::info!("restore superseded, dropping session");
            return;
        }
        self.activate(capability).await;
    }

    fn settle_discovery(&mut self, attempt: u64, info: ServerInfo) {
        match self.lifecycle.settle_auth(attempt) {
            Some(Settled::Awaited) => self.session.show_credentials(info),
            Some(Settled::Cancelled) => self.session.set_activity(LoginActivity::Idle),
            None => tracing::debug!("server info for a superseded attempt, dropping"),
        }
    }

    fn settle_auth_activity(&self, attempt: u64, activity: LoginActivity) {
        if self.lifecycle.awaits(attempt) {
            self.session.set_activity(activity);
        } else {
            tracing::debug!("activity update for a cancelled or superseded attempt, dropping");
        }
    }

    fn settle_auth_rejection(&mut self, attempt: u64, message: UserMessage) {
        self.session.finish_oauth();
        match self.lifecycle.settle_auth(attempt) {
            Some(Settled::Awaited) => self.session.fail_login(vec![message]),
            Some(Settled::Cancelled) => self.session.set_activity(LoginActivity::Idle),
            None => tracing::debug!("auth failure for a superseded attempt, dropping"),
        }
    }

    fn settle_auth_cancel(&mut self, attempt: u64) {
        self.session.finish_oauth();
        self.return_to_idle(attempt);
    }

    fn return_to_idle(&mut self, attempt: u64) {
        if self.lifecycle.settle_auth(attempt).is_some() {
            self.session.set_activity(LoginActivity::Idle);
        }
    }

    fn settle_erasure_start(&mut self, session: u64) {
        if self.lifecycle.begin_cleanup(session) {
            self.session.set_activity(LoginActivity::CleaningUp);
        }
    }

    fn settle_erasure(&mut self, session: u64, reason: EndReason, report: &CleanupReport) {
        if !self.lifecycle.finish_logout(session) {
            tracing::debug!("cleanup finished for a superseded session, dropping");
            return;
        }
        let mut messages = match reason {
            EndReason::Expired => vec![UserMessage::new(UserMessageKind::SessionExpired)],
            EndReason::UserLogout => Vec::new(),
        };
        messages.extend(session::cleanup_problem(report));
        self.session.settle_logout(messages);
    }

    async fn settle_login(&mut self, attempt: u64, established: EstablishedSession) {
        self.session.finish_oauth();
        self.session.spend_pending_passphrase();
        if self.lifecycle.promote_to_syncing(attempt).is_none() {
            if let Some(message) = undo_superseded_login(established).await {
                self.block_sign_in(message);
            } else {
                self.return_to_idle(attempt);
            }
            return;
        }
        self.activate(established.commit().await).await;
    }

    async fn settle_resume(&mut self, attempt: u64, capability: AuthenticatedSession) {
        self.session.finish_oauth();
        if self.lifecycle.resume_syncing(attempt).is_none() {
            tracing::info!("re-authentication superseded, releasing the session it produced");
            capability.lifecycle.suspend().await;
            self.return_to_idle(attempt);
            return;
        }
        self.activate(capability).await;
    }

    async fn activate(&mut self, capability: AuthenticatedSession) {
        let user_id = capability.session.user_id.clone();
        tracing::info!(%user_id, "authenticated");
        self.held_session = HeldSession::Running(Box::new(capability));
        self.emit_login_success(user_id);
        self.start_syncing().await;
    }

    async fn send_message(&mut self, room_id: RoomId, draft: MessageDraft) {
        let Some(timeline) = self.port(|a| &a.timeline) else {
            return;
        };
        self.submissions
            .send(&mut self.send_lanes, timeline, room_id.clone(), draft);
        self.return_to_live_for_send(room_id).await;
    }

    fn edit_message(&mut self, room_id: RoomId, edit: MessageEdit) {
        let Some(timeline) = self.port(|a| &a.timeline) else {
            return;
        };
        self.submissions
            .edit(&mut self.send_lanes, timeline, room_id, edit);
    }

    fn hold_unsaved_edit(&mut self, room_id: &RoomId, edit: MessageEdit) {
        if self.held_session.running().is_none() {
            tracing::debug!(%room_id, "dropping an unsaved edit from a session that ended");
            return;
        }
        self.submissions
            .unsaved(room_id, edit, self.selection.room.as_ref());
    }

    fn resolve_failed_send(&mut self, local_id: String, action: FailedSend) {
        let Some(timeline) = self.port(|a| &a.timeline) else {
            return;
        };
        let Some(room_id) = self.active_timeline.room_id().cloned() else {
            return;
        };
        self.active_timeline.spawn_resolve_failed_send(
            &mut self.operations,
            timeline,
            room_id,
            local_id,
            action,
        );
    }

    async fn send_sticker(
        &mut self,
        room_id: RoomId,
        pack: PackId,
        shortcode: String,
        reply_to: Option<String>,
    ) {
        let Some(stickers) = self.port(|a| &a.stickers) else {
            return;
        };
        self.stickers.send(
            &mut self.send_lanes,
            stickers,
            room_id.clone(),
            pack,
            shortcode,
            reply_to,
        );
        self.return_to_live_for_send(room_id).await;
    }

    async fn send_poll(&mut self, room_id: RoomId, draft: PollDraft) {
        let Some(timeline) = self.port(|a| &a.timeline) else {
            return;
        };
        polls::send(
            &mut self.send_lanes,
            Arc::clone(&self.output),
            timeline,
            room_id.clone(),
            draft,
        );
        self.return_to_live_for_send(room_id).await;
    }

    fn pick_attachment(&mut self, room_id: RoomId, pick: AttachmentPick) {
        self.attachments.pick(&mut self.operations, room_id, pick);
    }

    async fn send_attachment(
        &mut self,
        room_id: RoomId,
        caption: String,
        as_document: bool,
        reply_to: Option<String>,
    ) {
        let Some(timeline) = self.port(|a| &a.timeline) else {
            return;
        };
        let sent = self.attachments.send(
            &mut self.send_lanes,
            timeline,
            room_id.clone(),
            caption,
            as_document,
            reply_to,
        );
        if sent {
            self.return_to_live_for_send(room_id).await;
        }
    }

    fn media_in_active_room(&self) -> Option<(Arc<dyn MediaPort>, RoomId)> {
        let media = self.port(|a| &a.media)?;
        let room_id = self.active_timeline.room_id()?.clone();
        Some((media, room_id))
    }

    fn delete_message(&mut self, event_id: String) {
        let Some(port) = self.port(|a| &a.timeline) else {
            return;
        };
        let Some(room_id) = self.active_timeline.room_id().cloned() else {
            return;
        };
        self.message_actions
            .delete(&mut self.send_lanes, port, room_id, event_id);
    }

    fn change_pin(&mut self, event_id: String, change: PinChange) {
        let Some(port) = self.port(|a| &a.pinned) else {
            return;
        };
        let Some(room_id) = self.active_timeline.room_id().cloned() else {
            return;
        };
        self.pinned.change(port, room_id, event_id, change);
    }

    fn copy_message_link(&mut self, event_id: String) {
        let Some(port) = self.port(|a| &a.room_info) else {
            return;
        };
        let Some(room_id) = self.active_timeline.room_id().cloned() else {
            return;
        };
        self.message_actions
            .copy_link(&mut self.operations, port, room_id, event_id);
    }

    fn open_media(&mut self, event_id: String) {
        if let Some((media, room_id)) = self.media_in_active_room() {
            self.media.open_media(media, room_id, event_id);
        }
    }

    fn open_video(&mut self, event_id: String) {
        let Some((media, room_id)) = self.media_in_active_room() else {
            return;
        };
        if cfg!(feature = "video") {
            self.video.open(media, room_id, event_id);
        } else {
            self.media.play_video_externally(media, room_id, event_id);
        }
    }

    fn play_audio(&mut self, event_id: String) {
        if cfg!(feature = "video") {
            self.audio.play(&self.active_timeline, event_id);
        } else if let Some((media, room_id)) = self.media_in_active_room() {
            self.media.play_audio_externally(media, room_id, event_id);
        }
    }

    fn settle_audio_lookup(
        &mut self,
        room_id: &RoomId,
        generation: i32,
        request: u64,
        track: Option<Box<AudioTrack>>,
    ) {
        let Some(media) = self.port(|a| &a.media) else {
            return;
        };
        let track = track
            .filter(|_| self.active_timeline.is_current(room_id, generation))
            .map(|track| *track);
        self.audio.located(media, request, track);
    }

    fn save_file(&mut self, event_id: String, filename: String) {
        if let Some((media, room_id)) = self.media_in_active_room() {
            self.media.save_file(media, room_id, event_id, filename);
        }
    }

    async fn accept_verification(&mut self) {
        if let Some(verification) = self.port(|a| &a.verification) {
            self.verification
                .accept(&mut self.operations, verification)
                .await;
        }
    }

    async fn reject_verification(&mut self) {
        if let Some(verification) = self.port(|a| &a.verification) {
            self.verification
                .reject(&mut self.operations, verification)
                .await;
        }
    }

    async fn confirm_verification(&mut self) {
        if let Some(verification) = self.port(|a| &a.verification) {
            self.verification
                .confirm(&mut self.operations, verification)
                .await;
        }
    }

    async fn dismiss_verification(&mut self) {
        let verification = self.port(|a| &a.verification);
        self.verification
            .dismiss(&mut self.operations, verification)
            .await;
    }

    async fn select_room(&mut self, room_id: RoomId) {
        self.space_index.close();
        self.room_info.close();
        self.event_source.close();
        self.sync_selected_room(Some(&room_id));
        self.follow_pinned(room_id.clone());
        self.open_room(room_id, TimelineFocus::ReadPosition).await;
    }

    fn sync_selected_room(&self, room_id: Option<&RoomId>) {
        if let Some(sync) = self.port(|a| &a.sync) {
            sync.set_selected_room(room_id);
        }
    }

    fn follow_pinned(&mut self, room_id: RoomId) {
        if let Some(pinned) = self.port(|a| &a.pinned) {
            self.pinned.follow(pinned, room_id);
        }
    }

    fn open_pinned(&mut self, event_id: String) {
        self.pinned.opened(&event_id);
        self.active_timeline.jump_to_event(event_id);
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
        self.event_source.retarget(&self.active_timeline);
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
            meta,
            generation: self.selection.generation,
            live: self.active_timeline.is_live(),
        };
        if self.last_selected_room.as_ref() == Some(&next) {
            return;
        }
        self.emit_selected_room(next).await;
    }

    async fn drop_selected_room(&mut self) {
        self.room_info.close();
        self.event_source.close();
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
        self.stickers.clear_room();
        self.sync_selected_room(None);
        self.pinned.clear();
        self.active_timeline.clear_room(generation).await;
    }

    async fn start_syncing(&mut self) {
        let Some((sync, verification, lifecycle_port)) = self.held_session.running().map(|a| {
            (
                Arc::clone(&a.sync),
                Arc::clone(&a.verification),
                Arc::clone(&a.lifecycle),
            )
        }) else {
            tracing::debug!("no authenticated session to sync, ignoring");
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
            .spawn_listener(&mut self.background, verification);
        self.set_connection(ConnectionStatus::Connecting);
        RoomDirectory::spawn_sync_pipeline(
            &mut self.background,
            sync,
            Arc::clone(&self.output),
            self.events.clone(),
            self.dir_in_tx.clone(),
        );
        self.session
            .spawn_user_avatar_fetch(&mut self.background, lifecycle_port);
    }

    async fn shutdown_all_tasks(&mut self) {
        tokio::join!(
            self.background.shutdown(),
            self.active_timeline.shutdown(),
            self.pinned.restart(),
            self.operations.restart(),
            self.send_lanes.restart(),
            self.media.cancel_and_drain(),
            self.audio.restart(),
            self.video.restart(),
            self.stickers.restart(),
            self.space_index.restart(),
            self.room_info.restart(),
        );
    }

    async fn end_session(&mut self, reason: EndReason) {
        let Some(session) = self.lifecycle.begin_logout() else {
            return;
        };
        if matches!(reason, EndReason::Expired) {
            tracing::info!("session expired, clearing local state");
        }
        let ending = self.held_session.ending();
        self.tear_down_session(AppViewState::logged_out()).await;
        match ending {
            Some(Ending::Live(account, port)) if matches!(reason, EndReason::UserLogout) => {
                self.session
                    .spawn_logout(&mut self.operations, session, account, port);
            }
            Some(Ending::Live(account, port) | Ending::Detached(account, port)) => {
                self.session.spawn_erase_session(
                    &mut self.operations,
                    session,
                    account,
                    port,
                    reason,
                );
            }
            None => {
                self.lifecycle.finish_logout(session);
            }
        }
    }

    async fn suspend_session(&mut self) {
        let Some((session, lifecycle_port)) = self
            .held_session
            .running()
            .map(|a| (a.session.clone(), Arc::clone(&a.lifecycle)))
        else {
            return;
        };
        if !self.lifecycle.suspend() {
            return;
        }
        tracing::info!(
            user_id = %session.user_id,
            device_id = %session.device_id,
            "the homeserver asked for re-authentication, keeping this device and its local data"
        );
        self.tear_down_session(AppViewState::reauthenticating(&session))
            .await;
        lifecycle_port.suspend().await;
        self.held_session = HeldSession::Suspended(Box::new(SuspendedSession {
            account: AccountScope::from_session(&session),
            session,
            lifecycle: lifecycle_port,
        }));
    }

    async fn tear_down_session(&mut self, view: AppViewState) {
        self.attachments.clear();
        self.submissions.forget_all();
        self.message_actions.forget();
        self.event_source.reset();
        self.output
            .emit(Effect::SessionReset(Box::new(view.clone())))
            .await;
        self.output.replace(view);
        self.shutdown_all_tasks().await;
        self.media.clear_session().await;
        self.room_directory.reset();
        self.verification.reset();
        self.selection = Selection::default();
        self.last_selected_room = None;
        self.held_session = HeldSession::None;
    }

    async fn handle_quit(&mut self) {
        tokio::join!(
            self.background.shutdown(),
            self.active_timeline.shutdown(),
            self.pinned.shutdown(),
            self.operations.shutdown(),
            self.send_lanes.shutdown(),
            self.media.drain(),
            self.audio.shutdown(),
            self.video.shutdown(),
            self.stickers.shutdown(),
            self.space_index.shutdown(),
            self.room_info.shutdown(),
        );
    }
}
