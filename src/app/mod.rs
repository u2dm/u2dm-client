mod account;
mod active_timeline;
mod attachments;
mod audio;
mod calls;
mod conclude;
mod credentials;
mod encryption;
mod establish;
mod event;
mod event_source;
mod identity_alerts;
pub mod input;
mod invite_people;
mod invites;
mod lifecycle;
mod link_requests;
mod media;
mod mentions;
mod message_actions;
mod notifications;
mod packs;
mod pinned;
mod polls;
mod presence;
mod readers;
mod recover;
mod removals;
mod room_creation;
mod room_directory;
mod room_info;
mod room_log;
mod room_preview;
mod selection;
mod send_lanes;
mod session;
mod space_index;
mod space_leave;
mod space_order;
mod submissions;
mod task_group;
mod typing;
mod uia;
mod upgrades;
mod user_info;
mod verification;
mod video;

use std::path::PathBuf;
use std::sync::Arc;

use account::{Account, LogoutDecision, Settled as AccountSettled};
use active_timeline::ActiveTimeline;
use attachments::Attachments;
use audio::AudioController;
use calls::Calls;
use encryption::Encryption;
use establish::{EstablishedSession, Rollback};
use event::{
    AccountEvent, AppEvent, CallEvent, CreationSettled, DepartureEvent, EncryptionEvent, EndReason,
    IdentityEvent, InvitePeopleEvent, MentionsEvent, MessageActionEvent, PacksEvent, PresenceEvent,
    PreviewEvent, ReadersEvent, RoomActionEvent, SessionEvent, TimelineEvent, UserInfoEvent,
};
use event_source::EventSourceViewer;
use identity_alerts::{IdentityAlerts, Resolution};
use input::{CommandSender, EventSender, Inbox, Input};
use invite_people::InvitePeople;
use invites::{Answer, Arrival, Invites};
use lifecycle::{Lifecycle, Settled};
use media::MediaActions;
use mentions::{MentionSuggestions, Suggestion};
use message_actions::MessageActions;
use notifications::DesktopNotifications;
use packs::Packs;
use pinned::{PinDirection, PinnedMessages};
use presence::Presence;
use readers::ReaderList;
use recover::Recovery;
use removals::Removals;
use room_creation::{Created, RoomCreation};
use room_directory::{InvitesFold, RoomDirectory, RoomMeta};
use room_info::{ActionOutcome, RoomInfo, RosterOutcome};
use room_log::RoomLogViewer;
use room_preview::RoomPreviews;
use selection::Selection;
use send_lanes::SendLanes;
use session::{EndingSession, SessionController, SuspendedSession};
use space_index::{ChildTarget, JoinOutcome, ListedSpace, PageOutcome, SpaceIndex};
use space_leave::SpaceLeave;
use submissions::Submissions;
use task_group::TaskGroup;
use tokio::sync::{mpsc, watch};
use typing::Typing;
use uia::Reply;
use upgrades::{RoomUpgrades, Toward, UpgradeJoined};
use user_info::{ChatReady, UserInfo};
use verification::VerificationController;
use video::VideoController;

use crate::commands::effects::Effect;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::sync::DirectoryUpdate;
use crate::commands::ui::{
    ComposerActivity, MessageDraft, TimelineVisibility, UiCommand, ViewportChanged, WindowAttention,
};
use crate::commands::view::{AppViewState, LoginActivity, LoginStep, Toast};
use crate::domain::account::AccountScope;
use crate::domain::auth::ServerInfo;
use crate::domain::call::DeviceId;
use crate::domain::departure::RemovalList;
use crate::domain::emote::ComposerText;
use crate::domain::encryption::RecoveryKey;
use crate::domain::image_pack::PackId;
use crate::domain::invite::InviteList;
use crate::domain::matrix_link::{AddressEntry, MatrixLink, RoomAddress, RoomLink};
use crate::domain::media::AttachmentPick;
use crate::domain::message::{MessageEdit, PinChange};
use crate::domain::poll::PollDraft;
use crate::domain::push::PushNotice;
use crate::domain::room::{NotifyMode, RoomId, RoomList, Space};
use crate::domain::room_creation::{RoomDraft, RoomKind};
use crate::domain::room_search::TitleQuery;
use crate::domain::sync::{ConnectionStatus, ReauthReason};
use crate::domain::timeline::{AudioTrack, FailedSend, MessageReaders, TimelineFocus};
use crate::domain::user_info::{IgnoreChange, Moderation, UserId, server_part};
use crate::domain::verification::VerificationEvent;
use crate::ports::browser::BrowserPort;
use crate::ports::call::CallMediaPort;
use crate::ports::matrix::{AuthPort, AuthenticatedSession, CleanupReport, MediaPort};
use crate::ports::media::MediaFilePort;
use crate::ports::output::AppOutputPort;
use crate::ports::room_log::RoomLogPort;
use crate::ports::storage::StoragePort;

enum LinkTarget {
    Room(RoomId),
    Invite(RoomId),
    Space(RoomId),
}

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

async fn undo_superseded_login(established: EstablishedSession) -> Rollback {
    tracing::info!("authentication superseded, undoing the login");
    let rollback = established.roll_back().await;
    if let Rollback::Complete { report, .. } = &rollback {
        warn_if_not_undone(report);
    }
    rollback
}

fn warn_if_not_undone(report: &CleanupReport) {
    if !report.is_clean() {
        tracing::warn!("superseded login not fully undone: {}", report.summary());
    }
}

async fn say_goodbye_then_stop_syncing(
    presence: &mut Presence,
    calls: &mut Calls,
    background: &mut TaskGroup,
) {
    tokio::join!(presence.shutdown(), calls.shutdown());
    background.shutdown().await;
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
            cmd @ (UiCommand::OpenRemoval(_)
            | UiCommand::CloseRemoval
            | UiCommand::ForgetRoom(_)
            | UiCommand::OpenLeaveSpace(_)
            | UiCommand::CloseLeaveSpace
            | UiCommand::LeaveSpace { .. }) => self.handle_departure_command(cmd),
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
            cmd @ (UiCommand::StartIdentityReset
            | UiCommand::ResetIdentity
            | UiCommand::StartKeyExport
            | UiCommand::ExportRoomKeys(_)
            | UiCommand::StartKeyImport
            | UiCommand::ImportRoomKeys(_)) => self.handle_key_command(cmd),
            cmd @ (UiCommand::OpenAccount
            | UiCommand::CloseAccount
            | UiCommand::ShowAccountOverview
            | UiCommand::RenameSession { .. }
            | UiCommand::SignOutSession(_)
            | UiCommand::OpenAccountPage(_)
            | UiCommand::StartPasswordChange
            | UiCommand::ChangePassword { .. }
            | UiCommand::StartDeactivation
            | UiCommand::DeactivateAccount { .. }) => self.handle_account_command(cmd),
            cmd @ (UiCommand::AnswerAuthPassword(_)
            | UiCommand::ContinueAuth
            | UiCommand::ReopenAuthPage
            | UiCommand::CancelAuth) => self.handle_auth_command(cmd),
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
            UiCommand::SaveFile { event_id, filename } => {
                self.save_file(event_id, filename);
            }
            cmd @ (UiCommand::DeclineCall(_)
            | UiCommand::SilenceRing
            | UiCommand::OpenCallSheet { .. }
            | UiCommand::CloseCallSheet
            | UiCommand::StartCall { .. }
            | UiCommand::JoinCall { .. }
            | UiCommand::AnswerCall { .. }
            | UiCommand::LeaveCall
            | UiCommand::RetryFailedCall
            | UiCommand::DismissFailedCall
            | UiCommand::AnnounceCallAgain
            | UiCommand::SetMicrophone(_)
            | UiCommand::SetCamera(_)
            | UiCommand::ShareScreen(_)
            | UiCommand::UseCallDevice { .. }
            | UiCommand::RefreshCallDevices
            | UiCommand::FocusCallTile(_)
            | UiCommand::ShowCallFocus
            | UiCommand::ShowCallGallery
            | UiCommand::ViewCallShare
            | UiCommand::ShowSelfView(_)) => {
                self.handle_call_command(cmd);
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

    fn emit_login_success(&self, user_id: String, device_id: String) {
        self.output.publish(Box::new(move |view| {
            view.lifecycle.user_id = user_id;
            view.lifecycle.device_id = device_id;
            view.lifecycle.step = LoginStep::LoggedIn;
            view.lifecycle.activity = LoginActivity::Idle;
        }));
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

    async fn handle_invites_updated(&mut self, invites: InviteList) {
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

    async fn handle_spaces_updated(&mut self, spaces: Arc<[Space]>) {
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

    async fn follow_invites(&mut self) {
        let membership = self.room_directory.membership(&self.selection);
        let arrival = self
            .invites
            .follow(self.room_directory.invites(), &membership);
        self.open_arrival(arrival).await;
    }

    fn answer_invite(&mut self, room_id: &RoomId, answer: Answer) {
        if let Some(port) = self.port(|a| &a.invites) {
            self.invites
                .answer(port, self.room_directory.invite(room_id), answer);
        }
    }

    async fn settle_invite_answer(
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

    fn join_upgrade_link(&mut self, room_id: &RoomId, toward: Toward) {
        let Some(port) = self.port(|a| &a.upgrades) else {
            return;
        };
        let membership = self.room_directory.membership(&self.selection);
        self.upgrades
            .join(port, toward, self.room_directory.room(room_id), &membership);
        self.refresh_upgrades();
    }

    async fn settle_upgrade_join(&mut self, joined: UpgradeJoined) {
        let membership = self.room_directory.membership(&self.selection);
        let arrival = self
            .upgrades
            .settled(joined, &membership, self.selection.room.as_ref());
        self.open_upgraded(arrival).await;
    }

    async fn follow_upgrades(&mut self) {
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

    fn refresh_upgrades(&mut self) {
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

    async fn route_room_link(&mut self, link: RoomLink) {
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

    async fn open_external_link(&mut self, link: MatrixLink) {
        self.room_previews.hold(link);
        self.release_held_link().await;
    }

    async fn release_held_link(&mut self) {
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

    async fn find_room(&mut self, entry: AddressEntry) {
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

    async fn open_linked_room(&mut self, room_id: RoomId, focus: TimelineFocus) {
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

    async fn follow_previews(&mut self) {
        let membership = self.room_directory.membership(&self.selection);
        let resolved = self.room_previews.follow(&membership);
        self.open_previewed(resolved).await;
    }

    fn join_previewed_room(&mut self, address: &str) {
        let Some(port) = self.port(|a| &a.room_previews) else {
            return;
        };
        let membership = self.room_directory.membership(&self.selection);
        self.room_previews.join(port, address, &membership);
    }

    fn retry_room_preview(&mut self) {
        if let Some(port) = self.port(|a| &a.room_previews) {
            self.room_previews.retry(port);
        }
    }

    async fn handle_preview(&mut self, event: PreviewEvent) {
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
                self.room_previews.avatars_landed(generation, ready);
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

    fn open_room_creation(&mut self, kind: RoomKind) {
        let Some(running) = self.held_session.running() else {
            return;
        };
        let server = server_part(&running.session.user_id).to_owned();
        let parent = self
            .listed_space()
            .filter(|space| self.room_directory.space_powers(&space.id).add_children);
        self.room_creation.open(kind, parent, server);
    }

    fn create_room(&mut self, draft: &RoomDraft) {
        if let Some(port) = self.port(|a| &a.room_creation) {
            self.room_creation.create(port, draft);
        }
    }

    async fn settle_room_creation(&mut self, settled: CreationSettled) {
        let membership = self.room_directory.membership(&self.selection);
        let created = self.room_creation.settled(settled, &membership);
        self.open_created(created).await;
    }

    async fn follow_creations(&mut self) {
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

    fn open_invite_people(&mut self, room_id: &RoomId) {
        let target = self.room_directory.invite_target(room_id);
        self.invite_people.open(target);
    }

    fn search_people(&mut self, term: &str) {
        if let Some(port) = self.port(|a| &a.invite_people) {
            self.invite_people.search(port, term);
        }
    }

    fn retry_people_search(&mut self) {
        if let Some(port) = self.port(|a| &a.invite_people) {
            self.invite_people.retry(port);
        }
    }

    fn send_invites(&mut self, reason: &str) {
        if let Some(port) = self.port(|a| &a.invite_people) {
            self.invite_people.send(port, reason);
        }
    }

    fn follow_invite_people(&mut self) {
        let membership = self.room_directory.membership(&self.selection);
        self.invite_people.follow(&membership);
    }

    fn handle_invite_people(&mut self, event: InvitePeopleEvent) {
        let Some(port) = self.port(|a| &a.invite_people) else {
            return;
        };
        match event {
            InvitePeopleEvent::Found {
                generation,
                outcome,
            } => self.invite_people.found(port, generation, outcome),
            InvitePeopleEvent::AvatarsReady { generation, ready } => {
                self.invite_people.avatars_landed(generation, ready);
            }
            InvitePeopleEvent::Sent { batch, outcomes } => {
                if let Some(room_id) = self.invite_people.sent(batch, outcomes)
                    && let Some(room_info) = self.port(|a| &a.room_info)
                {
                    self.room_info.members_changed(room_info, &room_id);
                }
            }
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

    fn filter_rooms(&mut self, query: &str) {
        if self.room_directory.search(TitleQuery::new(query)) {
            self.room_directory.emit_rooms(&self.selection);
        }
    }

    fn fold_invites(&mut self, fold: InvitesFold) {
        if self.room_directory.fold_invites(fold) {
            self.room_directory.emit_rooms(&self.selection);
        }
    }

    fn open_space(&mut self, space: &RoomId) {
        let Some(route) = self.room_directory.rail_route(space) else {
            tracing::debug!(%space, "ignoring an open for a space the rail does not show");
            return;
        };
        self.selection.set_space(Some(route.space));
        self.selection.set_subspace(route.subspace);
        self.show_selected_scope();
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

    fn show_room_info_pane(&mut self) {
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

    fn retarget_room_info(&mut self, room_id: &RoomId) {
        match self.port(|a| &a.room_info) {
            Some(port) => self
                .room_info
                .follow_selection(port, self.room_directory.room(room_id)),
            None => self.room_info.close(),
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
        if let Some(port) = self.port(|a| &a.leaves) {
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

    fn handle_departure_command(&mut self, cmd: UiCommand) {
        match cmd {
            UiCommand::OpenRemoval(room_id) => {
                self.removals.open(self.room_directory.removal(&room_id));
            }
            UiCommand::CloseRemoval => self.removals.close(),
            UiCommand::ForgetRoom(room_id) => {
                if let Some(port) = self.port(|a| &a.leaves) {
                    self.removals
                        .forget(port, self.room_directory.removal(&room_id));
                }
            }
            UiCommand::OpenLeaveSpace(space_id) => {
                if let Some(port) = self.port(|a| &a.leaves) {
                    let departure = self.room_directory.space_departure(&space_id);
                    self.space_leave
                        .open(port, self.room_directory.space(&space_id), departure);
                }
            }
            UiCommand::CloseLeaveSpace => self.space_leave.close(),
            UiCommand::LeaveSpace {
                space_id,
                with_rooms,
            } => {
                if let Some(port) = self.port(|a| &a.leaves) {
                    if with_rooms {
                        for room_id in self.space_leave.departing_rooms(&space_id) {
                            self.calls.left_room(&room_id, &self.room_directory);
                        }
                    }
                    self.space_leave.leave(port, &space_id, with_rooms);
                }
            }
            other => tracing::debug!(command = %other, "not a departure command"),
        }
    }

    fn handle_departure_event(&mut self, event: DepartureEvent) {
        match event {
            DepartureEvent::RemovalForgotten {
                room_id,
                name,
                outcome,
            } => {
                if self.removals.forgotten(&room_id, &name, outcome) {
                    self.room_directory.forgot(&room_id);
                    self.room_directory.emit_directory(&self.selection);
                }
            }
            DepartureEvent::StandingRead {
                generation,
                standing,
            } => self.space_leave.standing_read(generation, standing),
            DepartureEvent::SpaceLeft {
                space_id,
                name,
                outcome,
            } => self.space_leave.left(&space_id, &name, outcome),
        }
    }

    fn handle_removals_updated(&mut self, removals: RemovalList) {
        if self.held_session.running().is_none() {
            return;
        }
        if self.room_directory.store_removals(removals) {
            self.room_directory.emit_directory(&self.selection);
            self.follow_removals();
        }
    }

    fn follow_removals(&mut self) {
        self.removals.follow(self.room_directory.removals());
    }

    fn follow_space_leave(&mut self) {
        let Some(space_id) = self.space_leave.shown_space() else {
            return;
        };
        let departure = self.room_directory.space_departure(&space_id);
        self.space_leave
            .follow(self.room_directory.space(&space_id), departure);
    }

    fn open_room_log(&mut self, room_id: RoomId) {
        let name = self
            .room_directory
            .room(&room_id)
            .map(|room| room.display_name.clone())
            .unwrap_or_default();
        self.room_log.open(room_id, name);
    }

    fn page_readers(&mut self) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.readers.page(port);
        }
    }

    fn settle_readers_lookup(
        &mut self,
        room_id: RoomId,
        generation: i32,
        request: u64,
        readers: Option<Box<MessageReaders>>,
    ) {
        if !self.active_timeline.is_current(&room_id, generation) {
            return;
        }
        if let Some(port) = self.port(|a| &a.room_info) {
            self.readers.located(port, room_id, request, readers);
        }
    }

    fn suggest_mentions(&mut self, room_id: RoomId, query: String, recent: &[UserId]) {
        if self.selection.room.as_ref() != Some(&room_id) {
            return;
        }
        let Some(port) = self.port(|a| &a.room_info) else {
            return;
        };
        let Some(room) = self.room_directory.room(room_id.as_ref()) else {
            return;
        };
        let suggestion = Suggestion {
            joined_count: room.member_count,
            offers_room: room.message_permissions.notify_room,
            own_user: self
                .held_session
                .running()
                .map(|session| session.session.user_id.clone())
                .unwrap_or_default(),
            room_id,
            query,
            recent: recent.iter().map(ToString::to_string).collect(),
        };
        self.mentions.suggest(port, suggestion);
    }

    fn handle_mentions(&mut self, event: MentionsEvent) {
        match event {
            MentionsEvent::RosterLoaded {
                request,
                joined_count,
                roster,
            } => {
                if let Some(port) = self.port(|a| &a.room_info) {
                    self.mentions
                        .roster_loaded(port, request, joined_count, roster);
                }
            }
            MentionsEvent::AvatarsReady { request, ready } => {
                self.mentions.avatars_ready(request, ready);
            }
        }
    }

    fn handle_packs(&mut self, event: PacksEvent) {
        match event {
            PacksEvent::CatalogLoaded {
                generation,
                catalog,
            } => self.packs.catalog_loaded(generation, catalog),
            PacksEvent::ImagesReady { generation, ready } => {
                self.packs.images_ready(generation, ready);
            }
            PacksEvent::GlobalFailed { op, pack } => {
                if let Some(port) = self.port(|a| &a.image_packs) {
                    self.packs.global_failed(port, op, &pack);
                }
            }
        }
    }

    fn handle_identity(&mut self, event: IdentityEvent) {
        match event {
            IdentityEvent::RoomAlerts { watch, alerts } => {
                self.identity_alerts.room_alerts_changed(watch, alerts);
            }
            IdentityEvent::OwnIdentity { session, own } => {
                if self.lifecycle.runs(session) {
                    self.identity_alerts.own_identity_changed(own);
                } else {
                    tracing::debug!("own identity for a session that no longer runs, dropping");
                }
            }
            IdentityEvent::Settled { request, failure } => {
                if self.held_session.running().is_some() {
                    self.identity_alerts.settled(request, failure);
                }
            }
        }
    }

    fn handle_encryption(&mut self, event: EncryptionEvent) {
        match event {
            EncryptionEvent::Changed { session, state } => {
                if !self.lifecycle.runs(session) {
                    tracing::debug!("encryption state of a session that no longer runs, dropping");
                    return;
                }
                self.encryption.state_changed(state);
            }
            EncryptionEvent::IdentitySettled { session, outcome } => {
                if !self.lifecycle.runs(session) {
                    tracing::debug!("identity setup of a session that no longer runs, dropping");
                    return;
                }
                self.encryption.identity_settled(outcome.as_ref());
            }
            EncryptionEvent::ActionSettled { request, outcome } => {
                let Some(port) = self.port(|a| &a.encryption) else {
                    return;
                };
                self.encryption.settled(Some(port), request, outcome);
            }
        }
        self.raise_session_alert();
        self.calls
            .trust_changed(self.encryption.is_verified(), &self.room_directory);
        self.user_info
            .session_trust_changed(self.encryption.is_verified());
    }

    fn use_recovery_key(&mut self, key: RecoveryKey) {
        if let Some(port) = self.port(|a| &a.encryption) {
            self.encryption.use_key(port, key);
        }
    }

    fn verify_with_other_session(&mut self) {
        if let Some(port) = self.port(|a| &a.verification) {
            self.encryption.verify_with_other_session(port);
        }
    }

    fn set_up_recovery(&mut self) {
        if let Some(port) = self.port(|a| &a.encryption) {
            self.encryption.set_up_recovery(port);
        }
    }

    fn change_recovery_key(&mut self) {
        if let Some(port) = self.port(|a| &a.encryption) {
            self.encryption.change_key(port);
        }
    }

    fn retry_encryption_setup(&mut self) {
        if let Some(port) = self.port(|a| &a.encryption) {
            self.encryption.retry_setup(port);
            self.raise_session_alert();
        }
    }

    fn handle_key_command(&mut self, cmd: UiCommand) {
        let Some(port) = self.port(|a| &a.encryption) else {
            return;
        };
        match cmd {
            UiCommand::StartIdentityReset => self.encryption.start_reset(),
            UiCommand::ResetIdentity => self.encryption.reset_identity(port),
            UiCommand::StartKeyExport => self.encryption.start_key_export(),
            UiCommand::ExportRoomKeys(passphrase) => self.encryption.export_keys(port, passphrase),
            UiCommand::StartKeyImport => self.encryption.start_key_import(),
            UiCommand::ImportRoomKeys(passphrase) => self.encryption.import_keys(port, passphrase),
            other => tracing::warn!(command = %other, "this command is not about room keys"),
        }
    }

    fn handle_account_command(&mut self, cmd: UiCommand) {
        let Some(port) = self.port(|a| &a.account) else {
            return;
        };
        match cmd {
            UiCommand::OpenAccount => self.account.open(&port),
            UiCommand::CloseAccount => self.account.close(),
            UiCommand::ShowAccountOverview => self.account.show_overview(),
            UiCommand::RenameSession { device_id, name } => {
                self.account.rename_session(port, device_id, &name);
            }
            UiCommand::SignOutSession(device_id) => self.account.sign_out_session(port, device_id),
            UiCommand::OpenAccountPage(page) => self.account.open_account_page(port, page),
            UiCommand::StartPasswordChange => self.account.start_password_change(),
            UiCommand::ChangePassword {
                password,
                sign_out_others,
            } => self
                .account
                .change_password(port, password, sign_out_others),
            UiCommand::StartDeactivation => self.account.start_deactivation(port),
            UiCommand::DeactivateAccount { erase } => self.account.deactivate(port, erase),
            other => tracing::warn!(command = %other, "this command is not about the account"),
        }
    }

    fn answer_auth(&mut self, reply: Reply) {
        if self.account.awaits_auth() {
            if let Some(port) = self.port(|a| &a.account) {
                self.account.answer_auth(port, reply);
            }
        } else if self.encryption.awaits_auth()
            && let Some(port) = self.port(|a| &a.encryption)
        {
            self.encryption.answer_auth(port, reply);
        }
    }

    fn reopen_auth_page(&mut self) {
        if self.account.awaits_auth() {
            self.account.reopen_auth_page();
        } else {
            self.encryption.reopen_auth_page();
        }
    }

    fn cancel_auth(&mut self) {
        if self.account.awaits_auth() {
            self.account.cancel_auth();
        } else if let Some(port) = self.port(|a| &a.encryption) {
            self.encryption.cancel_auth(port);
        }
    }

    fn handle_auth_command(&mut self, cmd: UiCommand) {
        match cmd {
            UiCommand::AnswerAuthPassword(password) => {
                self.answer_auth(Reply::Password(password));
            }
            UiCommand::ContinueAuth => self.answer_auth(Reply::Continue),
            UiCommand::ReopenAuthPage => self.reopen_auth_page(),
            UiCommand::CancelAuth => self.cancel_auth(),
            other => tracing::warn!(command = %other, "this command is not about authentication"),
        }
    }

    async fn handle_logout_command(&mut self, cmd: UiCommand) {
        match cmd {
            UiCommand::Logout => match self.port(|a| &a.account) {
                Some(port) => self.account.check_logout(port),
                None => self.end_session(EndReason::UserLogout).await,
            },
            UiCommand::ConfirmLogout => {
                if self.account.confirm_logout() {
                    self.end_session(EndReason::UserLogout).await;
                }
            }
            UiCommand::CancelLogout => self.account.cancel_logout(),
            other => tracing::warn!(command = %other, "this command is not about logging out"),
        }
    }

    async fn handle_account(&mut self, event: AccountEvent) {
        if self.held_session.running().is_none() {
            return;
        }
        match event {
            AccountEvent::FactsRead { generation, facts } => {
                self.account.facts_read(generation, facts);
            }
            AccountEvent::SessionsChanged {
                generation,
                sessions,
            } => self.account.sessions_changed(generation, sessions),
            AccountEvent::Settled { request, outcome } => {
                let port = self.port(|a| &a.account);
                if let AccountSettled::Deactivated = self.account.settled(port, request, outcome) {
                    tracing::info!("the account was deactivated, clearing local state");
                    self.end_session(EndReason::Deactivated).await;
                }
            }
            AccountEvent::LogoutChecked { request, risks } => {
                if let LogoutDecision::Proceed = self.account.logout_checked(request, risks) {
                    self.end_session(EndReason::UserLogout).await;
                }
            }
        }
    }

    fn raise_session_alert(&mut self) {
        self.identity_alerts
            .session_alert_changed(self.encryption.alert());
    }

    fn resolve_identity_alert(&mut self, resolution: Resolution) {
        if let Some(port) = self.port(|a| &a.identity) {
            self.identity_alerts.resolve(port, resolution);
        }
    }

    fn follow_identity_alerts(&mut self, room_id: RoomId) {
        if let Some(port) = self.port(|a| &a.identity) {
            self.identity_alerts.follow(port, room_id);
        }
    }

    fn handle_readers(&mut self, event: ReadersEvent) {
        match event {
            ReadersEvent::Named { request, readers } => {
                if let Some(port) = self.port(|a| &a.room_info) {
                    self.readers.named(port, request, readers);
                }
            }
            ReadersEvent::AvatarsReady { request, ready } => {
                self.readers.avatars_ready(request, ready);
            }
        }
    }

    fn open_user_info(&mut self, user_id: UserId) {
        if let Some(port) = self.port(|a| &a.user_info) {
            self.user_info
                .open(port, self.selection.room.clone(), user_id);
        }
        self.follow_presence();
    }

    fn retry_user_info(&mut self) {
        if let Some(port) = self.port(|a| &a.user_info) {
            self.user_info.retry(port);
        }
    }

    fn set_ignored(&mut self, user_id: &UserId, change: IgnoreChange) {
        if let Some(port) = self.port(|a| &a.user_info) {
            self.user_info.set_ignored(port, user_id, change);
        }
    }

    fn moderate(&mut self, user_id: &UserId, action: Moderation, reason: Option<String>) {
        if let Some(port) = self.port(|a| &a.user_info) {
            self.user_info.moderate(port, user_id, action, reason);
        }
    }

    async fn message_user(&mut self, user_id: &UserId) {
        let Some(port) = self.port(|a| &a.user_info) else {
            return;
        };
        if let Some(room_id) = self.user_info.message(port, user_id) {
            self.select_room(room_id).await;
        }
    }

    fn verify_user(&mut self, user_id: &UserId) {
        let (Some(users), Some(verification)) =
            (self.port(|a| &a.user_info), self.port(|a| &a.verification))
        else {
            return;
        };
        self.user_info
            .verify(users, verification, user_id, self.room_directory.rooms());
    }

    async fn direct_chat_ready(&mut self, ready: ChatReady) {
        match ready {
            ChatReady::Open(room_id) => self.select_room(room_id).await,
            ChatReady::Verify(user_id) => {
                if let Some(port) = self.port(|a| &a.verification) {
                    self.user_info.request_verification(port, user_id);
                }
            }
        }
    }

    fn follow_room_info(&mut self) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info.follow(port, self.room_directory.rooms());
        }
    }

    #[allow(clippy::too_many_lines)]
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
            AppEvent::RoomRosterFetched {
                generation,
                outcome,
            } => self.settle_room_roster(generation, outcome),
            AppEvent::RoomAboutFetched { generation, about } => {
                self.room_info.about_fetched(generation, about);
            }
            AppEvent::RoomInfoAvatarsReady { generation, ready } => {
                self.room_info.avatars_ready(generation, ready);
            }
            AppEvent::RoomAction(event) => self.handle_room_action(event),
            AppEvent::MessageAction(event) => self.handle_message_action(event),
            AppEvent::UserInfo(event) => self.handle_user_info(event).await,
            AppEvent::Readers(event) => self.handle_readers(event),
            AppEvent::Mentions(event) => self.handle_mentions(event),
            AppEvent::Packs(event) => self.handle_packs(event),
            AppEvent::Identity(event) => self.handle_identity(event),
            AppEvent::Encryption(event) => self.handle_encryption(event),
            AppEvent::Account(event) => self.handle_account(event).await,
            AppEvent::RoomLogGrew => self.room_log.reread(),
        }
    }

    fn settle_room_roster(&mut self, generation: u64, outcome: RosterOutcome) {
        if let Some(port) = self.port(|a| &a.room_info) {
            self.room_info.roster_fetched(port, generation, outcome);
        }
    }

    async fn handle_user_info(&mut self, event: UserInfoEvent) {
        let Some(port) = self.port(|a| &a.user_info) else {
            return;
        };
        match event {
            UserInfoEvent::Read {
                generation,
                outcome,
            } => {
                self.user_info
                    .read_landed(port, generation, outcome, self.room_directory.rooms());
            }
            UserInfoEvent::PronounsFetched {
                generation,
                pronouns,
            } => {
                self.user_info.pronouns_landed(generation, pronouns);
            }
            UserInfoEvent::ProfileFetched {
                generation,
                profile,
            } => {
                self.user_info.profile_landed(port, generation, profile);
            }
            UserInfoEvent::AvatarReady { generation } => {
                self.user_info.avatar_ready(generation);
            }
            UserInfoEvent::DmStarted {
                user_id,
                name,
                outcome,
            } => {
                let ready =
                    self.user_info
                        .dm_started(user_id, &name, outcome, self.room_directory.rooms());
                if let Some(ready) = ready {
                    self.direct_chat_ready(ready).await;
                }
            }
            UserInfoEvent::TrustChanged { generation, trust } => {
                self.user_info.trust_changed(generation, trust);
            }
            UserInfoEvent::VerifySettled {
                user_id,
                name,
                outcome,
            } => {
                self.user_info.verify_settled(&user_id, &name, outcome);
            }
            UserInfoEvent::IgnoreSettled {
                user_id,
                name,
                change,
                outcome,
            } => {
                self.user_info
                    .ignore_settled(&user_id, &name, change, outcome);
            }
            UserInfoEvent::ModerationSettled {
                room_id,
                user_id,
                name,
                action,
                outcome,
            } => {
                self.user_info
                    .moderation_settled((room_id, user_id), &name, action, outcome);
            }
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
            TimelineEvent::ReadersLocated {
                room_id,
                generation,
                request,
                readers,
            } => self.settle_readers_lookup(room_id, generation, request, readers),
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
            SessionEvent::AuthRejected { attempt, messages } => {
                self.settle_auth_rejection(attempt, messages);
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
                left_behind,
                report,
            } => self.settle_erasure(session, reason, left_behind.as_deref(), &report),
            SessionEvent::TokensNotPersisted { session } => {
                self.settle_tokens_not_persisted(session);
            }
            SessionEvent::UserAvatar { session, path } => self.settle_user_avatar(session, path),
            SessionEvent::Resumed {
                attempt,
                capability,
            } => self.settle_resume(attempt, *capability).await,
            SessionEvent::Suspended(reason) => self.suspend_session(reason).await,
            SessionEvent::Expired => self.end_session(EndReason::Expired).await,
        }
    }

    fn settle_restore_progress(&self, activity: LoginActivity) {
        if self.lifecycle.is_restoring() {
            self.session.set_activity(activity);
        }
    }

    fn settle_restore_failure(&mut self, messages: Vec<UserMessage>) {
        if self.lifecycle.restore_failed() {
            self.session.show_login(messages);
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
        let Some(session) = self.lifecycle.restore_succeeded() else {
            tracing::info!("restore superseded, dropping session");
            return;
        };
        self.activate(capability, session).await;
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

    fn settle_auth_rejection(&mut self, attempt: u64, messages: Vec<UserMessage>) {
        self.session.finish_oauth();
        match self.lifecycle.settle_auth(attempt) {
            Some(Settled::Awaited) => self.session.fail_login(messages),
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

    fn settle_undone_login(&mut self, attempt: u64, left_behind: Option<&str>) {
        let Some(device_id) = left_behind else {
            self.return_to_idle(attempt);
            return;
        };
        if self.lifecycle.settle_auth(attempt).is_some() {
            self.session
                .fail_login(vec![session::left_behind_notice(device_id)]);
        }
    }

    fn settle_erasure_start(&mut self, session: u64) {
        if self.lifecycle.begin_cleanup(session) {
            self.session.set_activity(LoginActivity::CleaningUp);
        }
    }

    fn settle_erasure(
        &mut self,
        session: u64,
        reason: EndReason,
        left_behind: Option<&str>,
        report: &CleanupReport,
    ) {
        if !self.lifecycle.finish_logout(session) {
            tracing::debug!("cleanup finished for a superseded session, dropping");
            return;
        }
        let mut messages = match reason {
            EndReason::Expired => vec![UserMessage::new(UserMessageKind::SessionExpired)],
            EndReason::Deactivated => vec![UserMessage::new(UserMessageKind::AccountDeactivated)],
            EndReason::UserLogout => Vec::new(),
        };
        messages.extend(left_behind.map(session::left_behind_notice));
        messages.extend(session::cleanup_problem(report));
        self.session.settle_logout(messages);
    }

    fn settle_tokens_not_persisted(&self, session: u64) {
        if self.lifecycle.runs(session) {
            show_toast(
                self.output.as_ref(),
                Toast::Error(UserMessage::new(UserMessageKind::SessionSaveFailed)),
            );
        } else {
            tracing::debug!("token save failure for a session that no longer runs, dropping");
        }
    }

    fn settle_user_avatar(&self, session: u64, path: Option<PathBuf>) {
        if self.lifecycle.runs(session) {
            self.output
                .publish(Box::new(move |view| view.lifecycle.avatar_path = path));
        } else {
            tracing::debug!("user avatar for a session that no longer runs, dropping");
        }
    }

    async fn settle_login(&mut self, attempt: u64, established: EstablishedSession) {
        self.session.finish_oauth();
        self.session.spend_pending_passphrase();
        let Some(session) = self.lifecycle.promote_to_syncing(attempt) else {
            match undo_superseded_login(established).await {
                Rollback::Complete { left_behind, .. } => {
                    self.settle_undone_login(attempt, left_behind.as_deref());
                }
                Rollback::Unresolved(message) => self.block_sign_in(message),
            }
            return;
        };
        self.activate(established.commit().await, session).await;
    }

    async fn settle_resume(&mut self, attempt: u64, capability: AuthenticatedSession) {
        self.session.finish_oauth();
        let Some(session) = self.lifecycle.resume_syncing(attempt) else {
            tracing::info!("re-authentication superseded, releasing the session it produced");
            capability.lifecycle.suspend().await;
            self.return_to_idle(attempt);
            return;
        };
        self.activate(capability, session).await;
    }

    async fn activate(&mut self, capability: AuthenticatedSession, session: u64) {
        let user_id = capability.session.user_id.clone();
        let device_id = capability.session.device_id.clone();
        tracing::info!(%user_id, "authenticated");
        self.held_session = HeldSession::Running(Box::new(capability));
        self.emit_login_success(user_id, device_id);
        self.start_syncing(session).await;
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
        let Some(stickers) = self.port(|a| &a.image_packs) else {
            return;
        };
        self.packs.send(
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
        caption: ComposerText,
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

    fn set_pack_global(&mut self, pack: PackId, global: bool) {
        if let Some(port) = self.port(|a| &a.image_packs) {
            self.packs.set_global(port, pack, global);
        }
    }

    fn react_with_emote(&mut self, event_id: String, pack: &PackId, shortcode: String) {
        let Some(mxc) = self.packs.emote_mxc(pack, &shortcode) else {
            tracing::debug!(%event_id, %pack, "the reacted emote is no longer shown");
            return;
        };
        self.active_timeline
            .toggle_reaction(event_id, mxc, Some(shortcode));
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

    async fn settle_verification_flow(&mut self, session: u64, event: VerificationEvent) {
        if self.lifecycle.runs(session) {
            self.verification.flow_advanced(event).await;
        } else {
            tracing::debug!("verification update for a session that no longer runs, dropping");
        }
    }

    async fn settle_verification_failure(&self, session: u64, failure: UserMessageKind) {
        if self.lifecycle.runs(session) {
            self.verification.action_failed(failure).await;
        } else {
            tracing::debug!("verification failure for a session that no longer runs, dropping");
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
        self.select_room_at(room_id, TimelineFocus::ReadPosition)
            .await;
    }

    async fn select_room_at(&mut self, room_id: RoomId, focus: TimelineFocus) {
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

    async fn open_room_from(&mut self, opening: UiCommand) {
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

    fn settle_notice(&mut self, session: u64, notice: PushNotice) {
        if !self.lifecycle.runs(session) {
            return;
        }
        if notice.is_call() && self.calls.rings_in(&notice.room_id) {
            tracing::debug!(room_id = %notice.room_id, "the room rings; its call notice shows no message notification");
            return;
        }
        self.notifications.arrived(notice, &self.room_directory);
    }

    fn timeline_visibility_changed(&mut self, visibility: TimelineVisibility) {
        self.active_timeline.visibility_changed(visibility);
        self.notifications
            .on_screen(self.active_timeline.on_screen());
    }

    fn sync_selected_room(&self, room_id: Option<&RoomId>) {
        if let Some(sync) = self.port(|a| &a.sync) {
            sync.set_selected_room(room_id);
        }
    }

    fn follow_typing(&mut self, room_id: RoomId) {
        if let Some(typing) = self.port(|a| &a.typing) {
            self.typing.retarget(typing, room_id);
        }
    }

    fn attention_changed(&mut self, attention: WindowAttention) {
        self.notifications.focus_changed(attention.focus);
        self.calls
            .focus_changed(attention.focus, &self.room_directory);
        let port = self.port(|a| &a.presence);
        self.presence.attention_changed(port, attention);
    }

    fn follow_presence(&mut self) {
        let partner = self.selection.room.as_ref().and_then(|room_id| {
            let partner = self.room_directory.room(room_id)?.direct_partner.clone()?;
            Some((room_id.clone(), partner))
        });
        let person = self.user_info.person().cloned();
        let port = self.port(|a| &a.presence);
        self.presence.follow(port, partner, person);
    }

    fn follow_calls(&mut self) {
        self.calls
            .selected(self.selection.room.as_ref(), &self.room_directory);
    }

    fn handle_call(&mut self, event: CallEvent) {
        match event {
            CallEvent::Signal { session, signal } => {
                if self.lifecycle.runs(session) {
                    self.calls.signal(signal, &self.room_directory);
                }
            }
            CallEvent::Transports { session, found } => {
                if self.lifecycle.runs(session) {
                    self.calls.transports_found(found, &self.room_directory);
                }
            }
            CallEvent::RingWindow { ring, deadline } => {
                self.calls
                    .ring_window(&ring, deadline, &self.room_directory);
            }
            CallEvent::Declined { room_id, sent } => {
                if self.held_session.running().is_some() {
                    self.calls
                        .decline_settled(&room_id, sent, &self.room_directory);
                }
            }
            CallEvent::DevicesListed { listing, lists } => {
                self.calls
                    .devices_listed(listing, lists, &self.room_directory);
            }
            event => {
                if event.call().is_some_and(|call| self.calls.is_current(call)) {
                    self.calls.call_event(event, &self.room_directory);
                } else {
                    tracing::debug!(
                        event = event.label(),
                        "an event of a call that ended, dropping"
                    );
                }
            }
        }
    }

    fn handle_call_command(&mut self, cmd: UiCommand) {
        let directory = &self.room_directory;
        match cmd {
            UiCommand::DeclineCall(room_id) => self.calls.decline(&room_id, directory),
            UiCommand::SilenceRing => self.calls.silence(directory),
            UiCommand::OpenCallSheet { room_id, video } => {
                self.calls.open_sheet(&room_id, video, directory);
            }
            UiCommand::CloseCallSheet => self.calls.close_sheet(directory),
            UiCommand::StartCall {
                room_id,
                video,
                media,
                leaving,
            } => self
                .calls
                .start_call(room_id, video, media, leaving, directory),
            UiCommand::JoinCall {
                room_id,
                media,
                leaving,
            } => self.calls.join_call(room_id, media, leaving, directory),
            UiCommand::AnswerCall {
                room_id,
                camera,
                leaving,
            } => self.calls.answer(room_id, camera, leaving, directory),
            UiCommand::LeaveCall => self.calls.leave_call(directory),
            UiCommand::RetryFailedCall => self.calls.retry_failed(directory),
            UiCommand::DismissFailedCall => self.calls.dismiss_failed(directory),
            UiCommand::AnnounceCallAgain => self.calls.announce_again(directory),
            UiCommand::SetMicrophone(on) => self.calls.set_microphone(on, directory),
            UiCommand::SetCamera(on) => self.calls.set_camera(on, directory),
            UiCommand::ShareScreen(on) => self.calls.set_screen_share(on, directory),
            UiCommand::UseCallDevice { kind, key } => {
                self.calls.use_device(kind, key.as_ref(), directory);
            }
            UiCommand::RefreshCallDevices => self.calls.refresh_devices(),
            UiCommand::FocusCallTile(key) => self.calls.focus_call_tile(&key, directory),
            UiCommand::ShowCallFocus => self.calls.show_call_focus(directory),
            UiCommand::ShowCallGallery => self.calls.show_call_gallery(directory),
            UiCommand::ViewCallShare => self.calls.view_call_share(directory),
            UiCommand::ShowSelfView(on) => self.calls.show_self_view(on, directory),
            other => tracing::warn!(command = %other, "this command is not about calls"),
        }
    }

    fn close_user_info(&mut self) {
        self.user_info.close();
        self.follow_presence();
    }

    fn handle_presence(&mut self, event: PresenceEvent) {
        match event {
            PresenceEvent::Disabled { session } => {
                if self.lifecycle.runs(session) {
                    self.presence.disabled();
                }
            }
            PresenceEvent::Looked {
                watch,
                user_id,
                result,
            } => {
                if self.held_session.running().is_some() {
                    self.presence.looked(watch, user_id, result);
                }
            }
        }
    }

    fn composer_changed(&mut self, activity: &ComposerActivity) {
        let port = self.port(|a| &a.typing);
        self.typing
            .composer_changed(port, self.selection.room.as_ref(), activity);
    }

    fn follow_pinned(&mut self, room_id: RoomId) {
        if let Some(pinned) = self.port(|a| &a.pinned) {
            self.pinned.follow(pinned, room_id);
        }
    }

    fn open_pinned(&mut self, event_id: String) {
        let Some(room_id) = self.active_timeline.room_id() else {
            return;
        };
        if !self.pinned.contains(room_id, &event_id) {
            return;
        }
        self.active_timeline.jump_to_event(event_id);
    }

    fn step_pinned(&mut self, event_id: &str, direction: PinDirection) {
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

    async fn start_syncing(&mut self, session: u64) {
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

    fn begin_calls(&mut self, session: u64) {
        let Some((port, rtc, user, device)) = self.held_session.running().map(|a| {
            (
                Arc::clone(&a.call_signals),
                Arc::clone(&a.rtc_membership),
                UserId::new(a.session.user_id.clone()),
                DeviceId::new(a.session.device_id.clone()),
            )
        }) else {
            return;
        };
        self.calls
            .begin_session(&mut self.background, port, rtc, session, user, device);
        self.calls
            .trust_changed(self.encryption.is_verified(), &self.room_directory);
    }

    fn begin_presence(&mut self, session: u64) {
        let Some((port, own)) = self.held_session.running().map(|a| {
            (
                Arc::clone(&a.presence),
                UserId::new(a.session.user_id.clone()),
            )
        }) else {
            return;
        };
        self.presence.begin_session(port, session, own);
    }

    async fn shutdown_all_tasks(&mut self) {
        tokio::join!(self.presence.restart(), self.calls.restart());
        tokio::join!(
            self.background.shutdown(),
            self.active_timeline.shutdown(),
            self.pinned.restart(),
            self.identity_alerts.restart(),
            self.encryption.restart(),
            self.account.restart(),
            self.operations.restart(),
            self.send_lanes.restart(),
            self.media.cancel_and_drain(),
            self.audio.restart(),
            self.video.restart(),
            self.packs.restart(),
            self.space_index.restart(),
            self.invites.restart(),
            self.removals.restart(),
            self.space_leave.restart(),
            self.upgrades.restart(),
            self.room_previews.restart(),
            self.room_creation.restart(),
            self.invite_people.restart(),
            self.typing.restart(),
            self.room_info.restart(),
            self.user_info.restart(),
            self.readers.restart(),
            self.mentions.restart(),
            self.room_log.restart(),
        );
    }

    async fn end_session(&mut self, reason: EndReason) {
        let Some(session) = self.lifecycle.begin_logout() else {
            return;
        };
        if matches!(reason, EndReason::Expired) {
            tracing::info!("session expired, clearing local state");
        }
        match (self.held_session.ending(), reason) {
            (Some(Ending::Live(ending)), EndReason::UserLogout) => {
                self.presence.sign_off();
                self.calls.hang_up();
                self.tear_down_session(Box::new(AppViewState::signing_out()))
                    .await;
                self.session
                    .spawn_logout(&mut self.operations, session, ending);
            }
            (Some(Ending::Detached(ending)), EndReason::UserLogout) => {
                tracing::info!(
                    device_id = %ending.device_id,
                    "erasing a suspended session without a homeserver logout, so the homeserver keeps this device"
                );
                self.tear_down_session(Box::new(AppViewState::logged_out()))
                    .await;
                let left_behind = Some(ending.device_id.clone());
                self.session.spawn_erase_session(
                    &mut self.operations,
                    session,
                    ending,
                    reason,
                    left_behind,
                );
            }
            (
                Some(Ending::Live(ending) | Ending::Detached(ending)),
                EndReason::Expired | EndReason::Deactivated,
            ) => {
                self.tear_down_session(Box::new(AppViewState::logged_out()))
                    .await;
                self.session.spawn_erase_session(
                    &mut self.operations,
                    session,
                    ending,
                    reason,
                    None,
                );
            }
            (None, _) => {
                self.tear_down_session(Box::new(AppViewState::logged_out()))
                    .await;
                self.lifecycle.finish_logout(session);
            }
        }
    }

    async fn suspend_session(&mut self, reason: ReauthReason) {
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
            ?reason,
            "the homeserver refused this session's token, keeping this device and its local data"
        );
        self.tear_down_session(Box::new(AppViewState::reauthenticating(&session, reason)))
            .await;
        lifecycle_port.suspend().await;
        self.held_session = HeldSession::Suspended(Box::new(SuspendedSession {
            account: AccountScope::from_session(&session),
            session,
            lifecycle: lifecycle_port,
        }));
    }

    async fn tear_down_session(&mut self, view: Box<AppViewState>) {
        self.attachments.clear();
        self.submissions.forget_all();
        self.message_actions.forget();
        self.event_source.reset();
        self.output.emit(Effect::SessionReset(view.clone())).await;
        self.output.replace(*view);
        self.shutdown_all_tasks().await;
        self.media.clear_session().await;
        self.room_directory.reset();
        self.verification.reset();
        self.notifications.reset();
        self.selection = Selection::default();
        self.last_selected_room = None;
        self.held_session = HeldSession::None;
        self.room_log.forget();
    }

    async fn handle_quit(&mut self) {
        self.presence.sign_off();
        tokio::join!(
            Box::pin(say_goodbye_then_stop_syncing(
                &mut self.presence,
                &mut self.calls,
                &mut self.background
            )),
            self.active_timeline.shutdown(),
            self.pinned.shutdown(),
            self.identity_alerts.shutdown(),
            self.encryption.shutdown(),
            self.account.shutdown(),
            self.operations.shutdown(),
            self.send_lanes.shutdown(),
            self.media.drain(),
            self.audio.shutdown(),
            self.video.shutdown(),
            self.packs.shutdown(),
            self.space_index.shutdown(),
            self.invites.shutdown(),
            self.removals.shutdown(),
            self.space_leave.shutdown(),
            self.upgrades.shutdown(),
            self.room_previews.shutdown(),
            self.room_creation.shutdown(),
            self.invite_people.shutdown(),
            self.typing.shutdown(),
            self.room_info.shutdown(),
            self.user_info.shutdown(),
            self.readers.shutdown(),
            self.mentions.shutdown(),
            self.room_log.shutdown(),
        );
    }
}
