mod call_flow;
mod departures;
mod joins;
mod links;
mod messages;
mod navigation;
mod people;
mod room_panels;
mod security;
mod session_end;
mod settings;
mod sign_in;
mod syncing;

use std::sync::Arc;

use tokio::sync::{mpsc, watch};

use super::account::Account;
use super::active_timeline::ActiveTimeline;
use super::attachments::Attachments;
use super::audio::AudioController;
use super::calls::Calls;
use super::encryption::Encryption;
use super::event::AppEvent;
use super::event_source::EventSourceViewer;
use super::identity_alerts::{IdentityAlerts, Resolution};
use super::input::{CommandSender, EventSender, Inbox, Input};
use super::invite_people::InvitePeople;
use super::invites::{Answer, Invites};
use super::lifecycle::Lifecycle;
use super::media::MediaActions;
use super::mentions::MentionSuggestions;
use super::message_actions::MessageActions;
use super::notifications::DesktopNotifications;
use super::packs::Packs;
use super::pinned::{PinDirection, PinnedMessages};
use super::presence::Presence;
use super::readers::ReaderList;
use super::recover::Recovery;
use super::removals::Removals;
use super::room_creation::RoomCreation;
use super::room_directory::{InvitesFold, RoomDirectory, RoomMeta};
use super::room_info::RoomInfo;
use super::room_log::RoomLogViewer;
use super::room_preview::RoomPreviews;
use super::selection::Selection;
use super::send_lanes::SendLanes;
use super::session::{EndingSession, SessionController, SuspendedSession};
use super::settings::SettingsController;
use super::space_index::SpaceIndex;
use super::space_leave::SpaceLeave;
use super::submissions::Submissions;
use super::task_group::TaskGroup;
use super::typing::Typing;
use super::upgrades::{RoomUpgrades, Toward};
use super::user_info::UserInfo;
use super::verification::{VerificationAction, VerificationController};
use super::video::VideoController;
use super::{lifecycle, show_toast};
use crate::commands::messages::UserMessage;
use crate::commands::sync::DirectoryUpdate;
use crate::commands::ui::{
    ComposerActivity, TimelineVisibility, UiCommand, ViewportChanged, WindowAttention,
};
use crate::commands::view::Toast;
use crate::domain::account::AccountScope;
use crate::domain::message::PinChange;
use crate::domain::room::RoomId;
use crate::domain::room_creation::RoomKind;
use crate::domain::settings::Settings;
use crate::domain::timeline::FailedSend;
use crate::domain::user_info::{IgnoreChange, Moderation};
use crate::ports::browser::BrowserPort;
use crate::ports::call::CallMediaPort;
use crate::ports::matrix::{AuthPort, AuthenticatedSession};
use crate::ports::media::MediaFilePort;
use crate::ports::output::AppOutputPort;
use crate::ports::room_log::RoomLogPort;
use crate::ports::settings::SettingsPort;
use crate::ports::storage::StoragePort;

#[derive(PartialEq, Eq)]
struct EmittedRoom {
    id: RoomId,
    meta: RoomMeta,
    generation: i32,
    live: bool,
}

fn log_command(cmd: &UiCommand) {
    if let Some(room_id) = cmd.room_id() {
        tracing::info!(command = %cmd, %room_id, "handling command");
    } else {
        tracing::info!(command = %cmd, "handling command");
    }
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
    Live(EndingSession),
    Detached(EndingSession),
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
            Self::Running(capability) => Some(Ending::Live(EndingSession {
                account: AccountScope::from_session(&capability.session),
                device_id: capability.session.device_id.clone(),
                lifecycle: Arc::clone(&capability.lifecycle),
            })),
            Self::Suspended(suspended) => Some(Ending::Detached(EndingSession {
                account: suspended.account.clone(),
                device_id: suspended.session.device_id.clone(),
                lifecycle: Arc::clone(&suspended.lifecycle),
            })),
        }
    }

    fn suspended(&self) -> Option<&SuspendedSession> {
        match self {
            Self::Suspended(suspended) => Some(suspended),
            Self::None | Self::Running(_) => None,
        }
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
    identity_alerts: IdentityAlerts,
    encryption: Encryption,
    account: Account,
    settings: SettingsController,
    media: MediaActions,
    audio: AudioController,
    video: VideoController,
    packs: Packs,
    space_index: SpaceIndex,
    invites: Invites,
    removals: Removals,
    space_leave: SpaceLeave,
    upgrades: RoomUpgrades,
    room_previews: RoomPreviews,
    room_creation: RoomCreation,
    invite_people: InvitePeople,
    typing: Typing,
    presence: Presence,
    calls: Calls,
    notifications: DesktopNotifications,
    room_info: RoomInfo,
    user_info: UserInfo,
    message_actions: MessageActions,
    event_source: EventSourceViewer,
    readers: ReaderList,
    mentions: MentionSuggestions,
    room_log: RoomLogViewer,
    attachments: Attachments,
    submissions: Submissions,
    selection: Selection,
    last_selected_room: Option<EmittedRoom>,
    call_room_subscribed: Option<RoomId>,
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
        room_logs: Arc<dyn RoomLogPort>,
        call_media: Option<Arc<dyn CallMediaPort>>,
        settings: Arc<dyn SettingsPort>,
        loaded_settings: Settings,
        commands: &CommandSender,
        dir_in_tx: mpsc::UnboundedSender<DirectoryUpdate>,
        output: Arc<dyn AppOutputPort>,
    ) -> Self {
        let events = commands.events();
        Self {
            encryption: Encryption::new(
                Arc::clone(&output),
                events.clone(),
                Arc::clone(&media_files),
                Arc::clone(&browser),
            ),
            account: Account::new(Arc::clone(&output), events.clone(), Arc::clone(&browser)),
            settings: SettingsController::new(
                Arc::clone(&output),
                events.clone(),
                settings,
                loaded_settings,
            ),
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
            identity_alerts: IdentityAlerts::new(Arc::clone(&output), events.clone()),
            media: MediaActions::new(Arc::clone(&media_files), Arc::clone(&output)),
            audio: AudioController::new(Arc::clone(&output), events.clone()),
            video: VideoController::new(Arc::clone(&output), events.clone()),
            packs: Packs::new(Arc::clone(&output), events.clone()),
            space_index: SpaceIndex::new(Arc::clone(&output), events.clone()),
            invites: Invites::new(Arc::clone(&output), events.clone()),
            removals: Removals::new(Arc::clone(&output), events.clone()),
            space_leave: SpaceLeave::new(Arc::clone(&output), events.clone()),
            upgrades: RoomUpgrades::new(Arc::clone(&output), events.clone()),
            room_previews: RoomPreviews::new(Arc::clone(&output), events.clone()),
            room_creation: RoomCreation::new(Arc::clone(&output), events.clone()),
            invite_people: InvitePeople::new(Arc::clone(&output), events.clone()),
            typing: Typing::new(Arc::clone(&output), events.clone()),
            presence: Presence::new(Arc::clone(&output), events.clone()),
            calls: Calls::new(Arc::clone(&output), events.clone(), call_media),
            notifications: DesktopNotifications::new(Arc::clone(&output), events.clone()),
            room_info: RoomInfo::new(Arc::clone(&output), events.clone()),
            user_info: UserInfo::new(Arc::clone(&output), events.clone()),
            message_actions: MessageActions::new(Arc::clone(&output), events.clone()),
            event_source: EventSourceViewer::new(Arc::clone(&output)),
            readers: ReaderList::new(Arc::clone(&output), events.clone()),
            mentions: MentionSuggestions::new(Arc::clone(&output), events.clone()),
            room_log: RoomLogViewer::new(room_logs, Arc::clone(&output), events.clone()),
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
            call_room_subscribed: None,
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
        mut composer_in_rx: watch::Receiver<ComposerActivity>,
        mut attention_in_rx: watch::Receiver<WindowAttention>,
    ) {
        if let Recovery::Blocked(message) = self.session.recover_interrupted_logins().await {
            self.block_sign_in(message);
        }
        let mut dir_done = false;
        let mut scroll_done = false;
        let mut visibility_done = false;
        let mut composer_done = false;
        let mut attention_done = false;
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
                        Some(DirectoryUpdate::Invites(invites)) => {
                            self.handle_invites_updated(invites).await;
                        }
                        Some(DirectoryUpdate::Removals(removals)) => {
                            self.handle_removals_updated(removals);
                        }
                        Some(DirectoryUpdate::Spaces(spaces)) => {
                            self.handle_spaces_updated(spaces).await;
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
                        self.timeline_visibility_changed(visibility);
                    }
                }
                changed = composer_in_rx.changed(), if !composer_done => {
                    if changed.is_err() {
                        composer_done = true;
                    } else {
                        let activity = composer_in_rx.borrow_and_update().clone();
                        self.composer_changed(&activity);
                    }
                }
                changed = attention_in_rx.changed(), if !attention_done => {
                    if changed.is_err() {
                        attention_done = true;
                    } else {
                        let attention = *attention_in_rx.borrow_and_update();
                        self.attention_changed(attention);
                    }
                }
            }
            self.sync_call_room();
        }
    }

    async fn handle_input(&mut self, input: Input) -> bool {
        match input {
            Input::Ui(cmd) => {
                log_command(&cmd);
                self.dispatch(cmd).await
            }
            Input::Internal(event) => {
                tracing::debug!(event = event.label(), "handling app event");
                self.handle_event(event).await;
                false
            }
        }
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
            opening @ (UiCommand::SelectRoom(_)
            | UiCommand::OpenNotification(_)
            | UiCommand::OpenRoomLink(_)
            | UiCommand::OpenExternalLink(_)
            | UiCommand::FindRoom(_)) => {
                self.open_room_from(opening).await;
            }
            UiCommand::FilterRooms(query) => {
                self.filter_rooms(&query);
            }
            UiCommand::FoldInvites => {
                self.fold_invites(InvitesFold::Folded);
            }
            UiCommand::UnfoldInvites => {
                self.fold_invites(InvitesFold::Unfolded);
            }
            UiCommand::OpenSpace(space) => {
                self.open_space(&space);
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
            UiCommand::OpenInvite(room_id) => {
                self.invites.open(self.room_directory.invite(&room_id));
            }
            UiCommand::CloseInvite => {
                self.invites.close();
            }
            UiCommand::AcceptInvite(room_id) => {
                self.answer_invite(&room_id, Answer::Accept);
            }
            UiCommand::DeclineInvite(room_id) => {
                self.answer_invite(&room_id, Answer::Decline);
            }
            UiCommand::Departure(command) => self.handle_departure_command(command),
            UiCommand::JoinSuccessor(room_id) => {
                self.join_upgrade_link(&room_id, Toward::Successor);
            }
            UiCommand::JoinPredecessor(room_id) => {
                self.join_upgrade_link(&room_id, Toward::Predecessor);
            }
            UiCommand::OpenRoomFinder => self.room_previews.open_entry(),
            UiCommand::JoinPreviewedRoom(address) => self.join_previewed_room(&address),
            UiCommand::RetryRoomPreview => self.retry_room_preview(),
            UiCommand::CloseRoomPreview => self.room_previews.close(),
            UiCommand::OpenCreateRoom => self.open_room_creation(RoomKind::Room),
            UiCommand::OpenCreateSpace => self.open_room_creation(RoomKind::Space),
            UiCommand::CloseCreateRoom => self.room_creation.close(),
            UiCommand::CreateRoom(draft) => self.create_room(&draft),
            UiCommand::OpenInvitePeople(room_id) => self.open_invite_people(&room_id),
            UiCommand::CloseInvitePeople => self.invite_people.close(),
            UiCommand::SearchPeople(term) => self.search_people(&term),
            UiCommand::RetryPeopleSearch => self.retry_people_search(),
            UiCommand::PickInvitee(user_id) => self.invite_people.pick(&user_id),
            UiCommand::UnpickInvitee(user_id) => self.invite_people.unpick(&user_id),
            UiCommand::SendInvites(reason) => self.send_invites(&reason),
            UiCommand::OpenRoomInfo(room_id) => {
                self.open_room_info(&room_id);
            }
            UiCommand::CloseRoomInfo => {
                self.room_info.close_dialog();
            }
            UiCommand::ShowRoomInfoPane => {
                self.show_room_info_pane();
            }
            UiCommand::HideRoomInfoPane => {
                self.room_info.hide_pane();
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
                self.calls.left_room(&room_id, &self.room_directory);
                self.leave_room(&room_id);
            }
            UiCommand::OpenRoomMenu(room_id) => {
                self.room_info.open_menu(
                    self.room_directory.room(&room_id),
                    self.room_directory.space(&room_id),
                );
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
            UiCommand::OpenRoomLog(room_id) => {
                self.open_room_log(room_id);
            }
            UiCommand::CloseRoomLog => {
                self.room_log.close();
            }
            UiCommand::OpenUserInfo(user_id) => {
                self.open_user_info(user_id);
            }
            UiCommand::CloseUserInfo => {
                self.close_user_info();
            }
            UiCommand::RetryUserInfo => {
                self.retry_user_info();
            }
            UiCommand::MessageUser(user_id) => {
                self.message_user(&user_id).await;
            }
            UiCommand::VerifyUser(user_id) => {
                self.verify_user(&user_id);
            }
            UiCommand::IgnoreUser(user_id) => {
                self.set_ignored(&user_id, IgnoreChange::Ignore);
            }
            UiCommand::UnignoreUser(user_id) => {
                self.set_ignored(&user_id, IgnoreChange::Unignore);
            }
            UiCommand::KickUser { user_id, reason } => {
                self.moderate(&user_id, Moderation::Kick, reason);
            }
            UiCommand::BanUser { user_id, reason } => {
                self.moderate(&user_id, Moderation::Ban, reason);
            }
            UiCommand::UnbanUser(user_id) => {
                self.moderate(&user_id, Moderation::Unban, None);
            }
            UiCommand::AcceptIdentity(user_id) => {
                self.resolve_identity_alert(Resolution::Accept(user_id));
            }
            UiCommand::WithdrawVerification(user_id) => {
                self.resolve_identity_alert(Resolution::Withdraw(user_id));
            }
            UiCommand::AcknowledgeIdentityReset => {
                self.resolve_identity_alert(Resolution::AcknowledgeOwnReset);
            }
            UiCommand::OpenEncryption => self.encryption.open(),
            UiCommand::CloseEncryption => {
                let port = self.port(|a| &a.encryption);
                self.encryption.close(port);
            }
            UiCommand::ShowEncryptionOverview => self.encryption.show_overview(),
            UiCommand::EnterRecoveryKey => self.encryption.enter_key(),
            UiCommand::UseRecoveryKey(key) => self.use_recovery_key(key),
            UiCommand::VerifyWithOtherSession => self.verify_with_other_session(),
            UiCommand::SetUpRecovery => self.set_up_recovery(),
            UiCommand::ChangeRecoveryKey => self.change_recovery_key(),
            UiCommand::RetryEncryptionSetup => self.retry_encryption_setup(),
            UiCommand::Keys(command) => self.handle_key_command(command),
            UiCommand::Account(command) => self.handle_account_command(command),
            UiCommand::Settings(command) => self.handle_settings_command(command),
            UiCommand::Auth(command) => self.handle_auth_command(command),
            UiCommand::RetryTimeline => {
                self.retry_timeline().await;
            }
            UiCommand::SendMessage { room_id, draft } => {
                self.send_message(room_id, draft).await;
            }
            UiCommand::EditMessage { room_id, edit } => {
                self.edit_message(room_id, edit);
            }
            UiCommand::SuggestMentions {
                room_id,
                query,
                recent,
            } => {
                self.suggest_mentions(room_id, query, &recent);
            }
            UiCommand::EndMentions => {
                self.mentions.end();
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
            UiCommand::PreviousPinned { event_id } => {
                self.step_pinned(&event_id, PinDirection::Older);
            }
            UiCommand::NextPinned { event_id } => {
                self.step_pinned(&event_id, PinDirection::Newer);
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
            UiCommand::OpenReaders { event_id } => {
                self.readers.open(&self.active_timeline, event_id);
            }
            UiCommand::CloseReaders => {
                self.readers.close();
            }
            UiCommand::PageReaders => {
                self.page_readers();
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
                self.active_timeline.toggle_reaction(event_id, key, None);
            }
            UiCommand::ReactWithEmote {
                event_id,
                pack,
                shortcode,
            } => self.react_with_emote(event_id, &pack, shortcode),
            UiCommand::SetPackGlobal { pack, global } => self.set_pack_global(pack, global),
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
            UiCommand::SaveFile {
                event_id,
                filename,
                mimetype,
            } => {
                self.save_file(event_id, filename, mimetype);
            }
            UiCommand::Call(command) => self.calls.handle(command, &self.room_directory),
            UiCommand::DismissToast => {
                show_toast(self.output.as_ref(), Toast::None);
            }
            UiCommand::AcceptVerification => {
                self.act_on_verification(VerificationAction::Accept).await;
            }
            UiCommand::RejectVerification => {
                self.act_on_verification(VerificationAction::Reject).await;
            }
            UiCommand::ConfirmVerification => {
                self.act_on_verification(VerificationAction::Confirm).await;
            }
            UiCommand::DismissVerification => {
                self.dismiss_verification().await;
            }
            cmd @ (UiCommand::Logout | UiCommand::ConfirmLogout | UiCommand::CancelLogout) => {
                self.handle_logout_command(cmd).await;
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
        self.held_session.running().map(|a| Arc::clone(pick(a)))
    }

    async fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Session(event) => self.handle_session_event(event).await,
            AppEvent::Timeline(event) => self.handle_timeline_event(event).await,
            AppEvent::SpaceOrderWriteFailed { op, spaces, error } => {
                self.revert_space_orders(op, &spaces, &error);
            }
            AppEvent::VerificationFlow { session, event } => {
                self.settle_verification_flow(session, event).await;
            }
            AppEvent::VerificationActionFailed { session, failure } => {
                self.settle_verification_failure(session, failure).await;
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
            AppEvent::TypistsChanged { watch, typists } => {
                self.typing.typists_changed(watch, typists);
            }
            AppEvent::Presence(event) => self.handle_presence(event),
            AppEvent::Call(event) => self.handle_call(event),
            AppEvent::Notified { session, notice } => self.settle_notice(session, *notice),
            AppEvent::SpaceIndex(event) => self.handle_space_index(event),
            AppEvent::InviteAnswered {
                room_id,
                name,
                answer,
                outcome,
            } => {
                self.settle_invite_answer(&room_id, &name, answer, outcome)
                    .await;
            }
            AppEvent::Departure(event) => self.handle_departure_event(event),
            AppEvent::UpgradeJoinSettled(joined) => {
                self.settle_upgrade_join(joined).await;
            }
            AppEvent::Preview(event) => self.handle_preview(event).await,
            AppEvent::RoomCreation(settled) => self.settle_room_creation(settled).await,
            AppEvent::InvitePeople(event) => self.handle_invite_people(event),
            AppEvent::RoomInfo(event) => self.handle_room_info(event),
            AppEvent::RoomAction(event) => self.handle_room_action(event),
            AppEvent::MessageAction(event) => self.handle_message_action(event),
            AppEvent::UserInfo(event) => self.handle_user_info(event).await,
            AppEvent::Readers(event) => self.handle_readers(event),
            AppEvent::Mentions(event) => self.handle_mentions(event),
            AppEvent::Packs(event) => self.handle_packs(event),
            AppEvent::Identity(event) => self.handle_identity(event),
            AppEvent::Encryption(event) => self.handle_encryption(event),
            AppEvent::Account(event) => self.handle_account(event).await,
            AppEvent::Settings(event) => self.handle_settings_event(event),
            AppEvent::RoomLogGrew => self.room_log.reread(),
        }
    }
}
