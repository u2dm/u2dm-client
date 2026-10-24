use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::future::pending;
use std::mem;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{mpsc, watch};
use tokio::task::spawn_blocking;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::account::{self, AccountBook};
use super::calls::{self, Step};
use super::encryption::{self, EncryptionBook};
use super::identity::{self, IdentityBook, SendHold};
use super::{
    attachments, audio, create_room, data, departures, invite_people, invites, links, login,
    logout, media, message_menu, notifications, pinned, polls, powers, presence, reactions,
    receipts, room_info, source, space_index, stickers, sync, timeline, typing, upgrades,
    user_info, verification, videos,
};
use crate::adapters::draft_size::{self, MediaDraft, RoomFacts};
use crate::adapters::markdown::RoomMentions;
use crate::adapters::{body_emoji, video};
use crate::domain::auth::{AuthMethod, LoginCredentials, OAuthLoginData, ServerInfo, Session};
use crate::domain::call::identity::spellings;
use crate::domain::call::ring::{CallSignal, OutgoingRing, RingId};
use crate::domain::call::sfu::{GrantFailure, GrantRequest, GrantResult, SfuGrant};
use crate::domain::call::{
    DelayedLeave, DeviceStanding, KeySendReport, MemberKey, OutgoingKey, OwnMembership,
    RestartResult, ScheduleFailure, ScheduleResult, Secret, SentMembership, Transport,
};
use crate::domain::capabilities::{AccountFacts, AccountPage};
use crate::domain::departure::{AdminStanding, Removal};
use crate::domain::emote::Emote;
use crate::domain::encryption::{
    AccountEncryption, IdentityOutcome, KeyImport, KeyPassphrase, Recovery, RecoveryAttempt,
    RecoveryKey, RecoverySetup,
};
use crate::domain::event_size::{Encryption, EventIds, EventSize};
use crate::domain::identity::{OwnIdentity, RoomAlert};
use crate::domain::image_pack::{PackId, PackImage};
use crate::domain::invite::Invite;
use crate::domain::invite_people::{
    InviteFailure, InviteResult, PEOPLE_SEARCH_LIMIT, PeopleFound, PeopleSearchFailure, Person,
    SearchResult,
};
use crate::domain::link::LauncherSafeUrl;
use crate::domain::matrix_link::RoomAddress;
use crate::domain::media::{MediaRendition, OutgoingAttachment, WaveformNeed};
use crate::domain::member_name::{MemberName, NamedUser};
use crate::domain::mention::is_user_tag;
use crate::domain::message::{
    CustomEmoji, EditTarget, MessageBody, MessageEdit, MessagePreviewKind, PinChange,
    PinnedMessage, ReplyInfo, RichText, SendState, TimelineMessage,
};
use crate::domain::poll::{PollAction, PollDraft};
use crate::domain::presence::{Availability, BeaconResult, LookupResult};
use crate::domain::push::{NoticeContent, PushNotice, PushTweaks};
use crate::domain::room::{NotifyMode, Room, RoomId, Space};
use crate::domain::room_creation::{
    ChildLink, CreateFailure, CreateResult, CreatedRoom, JoinAccess, RoomBlueprint, RoomKind, Shape,
};
use crate::domain::room_info::{
    MemberRole, Reader, RoomAbout, RosterMember, RosterSection, sort_roster,
};
use crate::domain::room_preview::{PreviewFailure, PreviewMembership, PreviewResult};
use crate::domain::sessions::{LogoutRisks, OwnSessions};
use crate::domain::space_index::HierarchyPage;
use crate::domain::sync::{SyncEvent, SyncOutcome, SyncSupport};
use crate::domain::timeline::{
    Arrivals, AudioLookup, AudioTrack, EventSource, JumpTarget, MessageReaders,
    PaginationDirection, PaginationOutcome, TimelineCommand, TimelineFocus, TimelinePatch,
    TimelineUpdate, locate_audio,
};
use crate::domain::uia::{AccountPassword, AuthAnswer, Guarded};
use crate::domain::user_info::{
    GlobalProfile, IdentityTrust, IgnoreChange, Moderation, RoomMembership, UserId, UserProfile,
    UserTrust, localpart, server_part,
};
use crate::domain::verification::{VerificationCancellation, VerificationEvent};
use crate::error::{AppError, Result};
use crate::ports::matrix::{
    AccountPort, AttachmentHandoff, AuthPort, AuthenticatedSession, CallSignalPort, CleanupReport,
    DeviceRemoval, EncryptionPort, IdentityPort, ImagePackPort, InterruptedLogin, InvitePeoplePort,
    InvitePort, LeavePort, LocalDataOwnership, MediaPort, PackCatalog, PinnedPort, PresencePort,
    ProgressSink, PushPort, RestoreOutcome, RestoreStep, RoomCreationPort, RoomInfoPort,
    RoomPreviewPort, RtcMembershipPort, SessionPort, SpaceIndexPort, SpaceOrderPort, StoreAdoption,
    SyncPort, SyncSink, TimelinePort, TypingPort, UpgradePort, UserInfoPort, VerificationPort,
};
use crate::ports::media::MediaCache;
use crate::util::unix_now_ms;

const GRANT_REFUSED: u16 = 403;

pub struct DemoMatrix;

#[async_trait]
impl AuthPort for DemoMatrix {
    async fn discover_auth(&self, homeserver: &str, _passphrase: &str) -> Result<ServerInfo> {
        login::pause().await;
        let (auth_methods, unsupported_flows) = login::requested().map_or_else(
            || (vec![AuthMethod::Password], Vec::new()),
            |demo| (demo.methods.clone(), demo.unsupported_flows.clone()),
        );
        Ok(ServerInfo {
            auth_methods,
            unsupported_flows,
            homeserver_url: format!("https://{homeserver}"),
            sync: sync::support(),
        })
    }

    async fn login_password(&self, _creds: LoginCredentials) -> Result<Session> {
        login::pause().await;
        Ok(data::session())
    }

    async fn login_oauth_start(&self) -> Result<OAuthLoginData> {
        demo_oauth_start().await
    }

    async fn login_oauth_finish(&self) -> Result<Session> {
        if !login::oauth_succeeds() {
            return Err(unavailable("OAuth login"));
        }
        login::pause().await;
        Ok(oauth_session())
    }

    async fn adopt_session(
        &self,
        session: &Session,
        _passphrase: &str,
    ) -> Result<Box<dyn StoreAdoption>> {
        login::pause().await;
        Ok(Box::new(DemoAdoption {
            session: session.clone(),
            txn: "demo".to_owned(),
        }))
    }

    async fn cancel_oauth(&self) {}

    async fn restore_session(
        &self,
        session: &Session,
        _passphrase: &str,
        on_progress: ProgressSink,
    ) -> Result<RestoreOutcome> {
        for step in [RestoreStep::Connecting, RestoreStep::RestoringAuth] {
            on_progress(step);
            login::pause().await;
        }
        if login::identity_diverged() {
            tracing::info!(
                "demo mode: the homeserver publishes another signing key for this device"
            );
            return Ok(RestoreOutcome::Diverged(logout::answer().await));
        }
        Ok(RestoreOutcome::Restored(Box::new(authenticated(
            session.clone(),
        ))))
    }

    async fn reauthenticate(
        &self,
        prior: &Session,
        _passphrase: &str,
        _creds: LoginCredentials,
    ) -> Result<AuthenticatedSession> {
        login::pause().await;
        Ok(authenticated(prior.clone()))
    }

    async fn reauth_oauth_start(
        &self,
        _prior: &Session,
        _passphrase: &str,
    ) -> Result<OAuthLoginData> {
        demo_oauth_start().await
    }

    async fn reauth_oauth_finish(&self, prior: &Session) -> Result<AuthenticatedSession> {
        if !login::oauth_succeeds() {
            return Err(unavailable("OAuth login"));
        }
        login::pause().await;
        Ok(authenticated(prior.clone()))
    }

    fn local_data_ownership(&self) -> LocalDataOwnership {
        LocalDataOwnership::Exclusive
    }

    async fn interrupted_logins(&self) -> Result<Vec<InterruptedLogin>> {
        Ok(Vec::new())
    }

    async fn unwind_login(&self, _txn: &str) -> CleanupReport {
        CleanupReport::default()
    }

    async fn settle_login(&self, _txn: &str) -> CleanupReport {
        CleanupReport::default()
    }

    async fn mark_rolled_back(&self, _txn: &str) -> Result<()> {
        Ok(())
    }

    async fn forget_login(&self, _txn: &str) {}
}

struct DemoAdoption {
    session: Session,
    txn: String,
}

#[async_trait]
impl StoreAdoption for DemoAdoption {
    fn transaction(&self) -> &str {
        &self.txn
    }

    async fn credentials_staged(&self) -> Result<()> {
        Ok(())
    }

    async fn credentials_written(&self) -> Result<()> {
        Ok(())
    }

    async fn rolling_back(&self) -> Result<()> {
        Ok(())
    }

    async fn logout(&self) -> DeviceRemoval {
        logout::answer().await
    }

    async fn commit(self: Box<Self>) -> AuthenticatedSession {
        authenticated(self.session)
    }

    async fn unwind(self: Box<Self>) -> CleanupReport {
        CleanupReport::default()
    }
}

fn oauth_session() -> Session {
    Session {
        client_id: Some("demo-oauth-client".to_owned()),
        ..data::session()
    }
}

fn authenticated(session: Session) -> AuthenticatedSession {
    let authed = Arc::new(DemoAuthed::default());
    let sync = Arc::clone(&authed);
    let timeline = Arc::clone(&authed);
    let pinned = Arc::clone(&authed);
    let media = Arc::clone(&authed);
    let verification = Arc::clone(&authed);
    let identity = Arc::clone(&authed);
    let encryption = Arc::clone(&authed);
    let space_order = Arc::clone(&authed);
    let space_index = Arc::clone(&authed);
    let invites = Arc::clone(&authed);
    let leaves = Arc::clone(&authed);
    let upgrades = Arc::clone(&authed);
    let room_previews = Arc::clone(&authed);
    let room_creation = Arc::clone(&authed);
    let invite_people = Arc::clone(&authed);
    let typing = Arc::clone(&authed);
    let presence = Arc::clone(&authed);
    let call_signals = Arc::clone(&authed);
    let rtc_membership = Arc::clone(&authed);
    let push = Arc::clone(&authed);
    let room_info = Arc::clone(&authed);
    let user_info = Arc::clone(&authed);
    let image_packs = Arc::clone(&authed);
    let account = Arc::clone(&authed);
    AuthenticatedSession {
        session,
        sync,
        timeline,
        pinned,
        media,
        verification,
        identity,
        encryption,
        space_order,
        space_index,
        invites,
        leaves,
        upgrades,
        room_previews,
        room_creation,
        invite_people,
        typing,
        presence,
        call_signals,
        rtc_membership,
        push,
        room_info,
        user_info,
        image_packs,
        account,
        lifecycle: authed,
    }
}

struct ActiveRoom {
    room_id: RoomId,
    live: bool,
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    messages: Vec<TimelineMessage>,
    prepended: usize,
    receipts: Vec<receipts::Receipt>,
}

type SharedActiveRoom = Arc<Mutex<Option<ActiveRoom>>>;

#[derive(Default, Clone)]
struct RoomLists {
    joined: Vec<RoomId>,
    overrides: data::RoomOverrides,
}

impl RoomLists {
    fn rooms(&self) -> Vec<Arc<Room>> {
        data::rooms_with(&self.joined, &self.overrides)
    }

    fn spaces(&self) -> Vec<Space> {
        data::spaces_with(&self.joined, &self.overrides)
    }

    fn invites(&self) -> Vec<Arc<Invite>> {
        data::invites_with(&self.joined, &self.overrides)
    }

    fn removals(&self) -> Vec<Arc<Removal>> {
        data::removals_with(&self.overrides)
    }
}

fn current_lists(lists: &Mutex<RoomLists>) -> RoomLists {
    lists.lock().map(|lists| lists.clone()).unwrap_or_default()
}

struct Revision {
    body: MessageBody,
    body_emoji: BTreeMap<String, CustomEmoji>,
    mentions_room: bool,
    edited: bool,
}

impl Revision {
    fn restore(self, message: &mut TimelineMessage) {
        message.body = self.body;
        message.body_emoji = self.body_emoji;
        message.mentions_room = self.mentions_room;
        message.edited = self.edited;
    }
}

struct RevisedText {
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    row: usize,
    previous: Revision,
    message: TimelineMessage,
}

fn edit_names(target: &EditTarget, message: &TimelineMessage) -> bool {
    match target {
        EditTarget::Sent(event_id) => message.event_id.as_deref() == Some(event_id),
        EditTarget::Queued(local_id) => message.local_id.as_deref() == Some(local_id),
    }
}

fn revised_body(body: &MessageBody, text: &str, sent: RichText) -> Option<MessageBody> {
    match body {
        MessageBody::Text(_) => Some(MessageBody::Text(sent)),
        MessageBody::Image { meta, .. } => Some(MessageBody::Image {
            caption: revised_caption(text, sent),
            meta: meta.clone(),
        }),
        MessageBody::Video { meta, .. } => Some(MessageBody::Video {
            caption: revised_caption(text, sent),
            meta: meta.clone(),
        }),
        MessageBody::Audio { meta, .. } => Some(MessageBody::Audio {
            caption: revised_caption(text, sent),
            meta: meta.clone(),
        }),
        MessageBody::File { meta, .. } => Some(MessageBody::File {
            caption: revised_caption(text, sent),
            meta: meta.clone(),
        }),
        _ => None,
    }
}

fn revised_caption(text: &str, sent: RichText) -> Option<RichText> {
    (!text.is_empty()).then_some(sent)
}

#[derive(Default, Clone, PartialEq, Eq)]
enum OutgoingFlow {
    #[default]
    None,
    Session,
    Person(String),
}

struct OpenedWindow {
    all: Vec<TimelineMessage>,
    newer: usize,
    messages: Vec<TimelineMessage>,
    reset_messages: Vec<TimelineMessage>,
}

#[derive(Default)]
struct DemoAuthed {
    active: SharedActiveRoom,
    sent: AtomicU64,
    own_sends: Mutex<HashMap<RoomId, Vec<TimelineMessage>>>,
    arrived: Mutex<HashMap<RoomId, Vec<TimelineMessage>>>,
    verification_tx: Mutex<Option<mpsc::UnboundedSender<VerificationEvent>>>,
    sas_started: Arc<AtomicBool>,
    outgoing: Arc<Mutex<OutgoingFlow>>,
    flows: Arc<AtomicU64>,
    sync_sink: Mutex<Option<SyncSink>>,
    lists: Arc<Mutex<RoomLists>>,
    pin_boards: Mutex<HashMap<RoomId, watch::Sender<Vec<String>>>>,
    deleted: Arc<Mutex<HashSet<String>>>,
    ignored: Mutex<HashSet<String>>,
    memberships: Mutex<HashMap<(RoomId, String), RoomMembership>>,
    creations: AtomicU64,
    shared_packs: Mutex<HashMap<String, bool>>,
    identity: IdentityBook,
    encryption: EncryptionBook,
    account: Arc<AccountBook>,
    verified_people: user_info::VerifiedPeople,
    typing_notices: typing::NoticeClock,
    presence: presence::PresenceBook,
}

impl DemoAuthed {
    fn room_history(&self, room_id: &RoomId) -> Vec<TimelineMessage> {
        let mut history = data::messages(room_id);
        if let Ok(arrived) = self.arrived.lock()
            && let Some(messages) = arrived.get(room_id)
        {
            history.extend(messages.iter().cloned());
        }
        if let Ok(own_sends) = self.own_sends.lock()
            && let Some(sent) = own_sends.get(room_id)
        {
            history.extend(sent.iter().cloned());
        }
        if let Ok(deleted) = self.deleted.lock()
            && !deleted.is_empty()
        {
            history.retain(|message| {
                message
                    .event_id
                    .as_ref()
                    .is_none_or(|event_id| !deleted.contains(event_id))
            });
            for message in &mut history {
                stamp_deleted_reply(message, &deleted);
            }
        }
        if let Ok(ignored) = self.ignored.lock()
            && !ignored.is_empty()
        {
            history.retain(|message| !ignored.contains(&message.sender));
        }
        history
    }

    fn is_ignored(&self, user_id: &str) -> bool {
        self.ignored
            .lock()
            .is_ok_and(|ignored| ignored.contains(user_id))
    }

    fn replay_live_window(&self) {
        let Some((timeline_tx, messages)) = self.refiltered_window() else {
            return;
        };
        tokio::spawn(async move {
            sleep(room_info::ECHO_LAG).await;
            send_patch(&timeline_tx, TimelinePatch::Reset(messages)).await;
        });
    }

    fn refiltered_window(&self) -> Option<(mpsc::Sender<TimelineUpdate>, Vec<TimelineMessage>)> {
        let mut guard = self.active.lock().ok()?;
        let active = guard.as_mut().filter(|active| active.live)?;
        active.messages = self.room_history(&active.room_id);
        active.prepended = 0;
        Some((active.timeline_tx.clone(), active.messages.clone()))
    }

    fn send_state_in(&self, room_id: &RoomId) -> SendState {
        if self.holds_sends_in(room_id) {
            SendState::Failed
        } else {
            outgoing_send_state()
        }
    }

    fn holds_sends_in(&self, room_id: &RoomId) -> bool {
        let unverified_here =
            data::room_is_encrypted(room_id) && self.encryption.holds_encrypted_sends();
        unverified_here || matches!(self.identity.send_hold(room_id), SendHold::VerificationLost)
    }

    fn keep_own_send(&self, room_id: &RoomId, message: &TimelineMessage) {
        if let Ok(mut own_sends) = self.own_sends.lock() {
            own_sends
                .entry(room_id.clone())
                .or_default()
                .push(message.clone());
        }
    }

    fn settle_own_send(&self, room_id: &RoomId, local_id: &str, resend: bool) {
        let Ok(mut own_sends) = self.own_sends.lock() else {
            return;
        };
        let Some(sent) = own_sends.get_mut(room_id) else {
            return;
        };
        if resend {
            if let Some(message) = sent
                .iter_mut()
                .find(|m| m.local_id.as_deref() == Some(local_id))
            {
                mark_sent(message);
            }
        } else {
            sent.retain(|m| m.local_id.as_deref() != Some(local_id));
        }
    }

    fn keep_own_edit(&self, room_id: &RoomId, revised: &TimelineMessage) {
        let Ok(mut own_sends) = self.own_sends.lock() else {
            return;
        };
        let Some(sent) = own_sends
            .get_mut(room_id)
            .and_then(|sent| sent.iter_mut().find(|m| m.unique_id == revised.unique_id))
        else {
            return;
        };
        sent.body = revised.body.clone();
        sent.body_emoji = revised.body_emoji.clone();
        sent.mentions_room = revised.mentions_room;
        sent.edited = revised.edited;
    }

    fn revise_own_text(&self, room_id: &RoomId, edit: &MessageEdit) -> Option<RevisedText> {
        let mut guard = self.active.lock().ok()?;
        let active = guard.as_mut()?;
        if &active.room_id != room_id {
            return None;
        }
        let offset = active
            .messages
            .iter()
            .position(|message| edit_names(&edit.target, message))?;
        let row = active.prepended.saturating_add(offset);
        let message = active.messages.get_mut(offset)?;
        message.editable_text()?;
        let emotes = with_target_emotes(&edit.emotes, message.body.html());
        let sent = data::sent_text(&edit.body, &emotes);
        let revised = revised_body(&message.body, &edit.body, sent.text)?;
        let revised_emoji = media::emoji_states(&revised);
        let previous = Revision {
            body: mem::replace(&mut message.body, revised),
            body_emoji: mem::replace(&mut message.body_emoji, revised_emoji),
            mentions_room: message.mentions_room,
            edited: message.edited,
        };
        message.mentions_room = sent.mentions_room;
        message.edited |= edit.target.event_id().is_some();
        if !timeline::scenario().edits_fail {
            self.keep_own_edit(room_id, message);
        }
        Some(RevisedText {
            timeline_tx: active.timeline_tx.clone(),
            row,
            previous,
            message: message.clone(),
        })
    }

    fn reply_in_room(&self, active: &ActiveRoom, event_id: &str) -> Option<ReplyInfo> {
        reply_info(&active.messages, event_id)
            .or_else(|| reply_info(&self.room_history(&active.room_id), event_id))
    }

    async fn open_window(
        &self,
        room_id: &RoomId,
        focus: &TimelineFocus,
        scenario: timeline::Scenario,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) -> Result<Option<OpenedWindow>> {
        let Ok(reset_slot) = timeline_tx.reserve().await else {
            return Ok(None);
        };
        let Ok(mut active) = self.active.lock() else {
            return Err(unavailable("the demo timeline"));
        };
        let all = self.room_history(room_id);
        let window = opening_window(&all, focus, scenario)?;
        let newer = window.end;
        let messages = window_messages(&all, window, focus);
        let reset_messages = reset_messages(&messages);
        let reset = opening_patch(scenario, reset_messages.clone());
        reset_slot.send(TimelineUpdate::patch(reset));
        *active = Some(ActiveRoom {
            room_id: room_id.clone(),
            live: focus.is_live(),
            timeline_tx: timeline_tx.clone(),
            messages: messages.clone(),
            prepended: 0,
            receipts: receipts::seed_receipts(&all),
        });
        Ok(Some(OpenedWindow {
            all,
            newer,
            messages,
            reset_messages,
        }))
    }

    fn remove_from_window(
        &self,
        room_id: &RoomId,
        event_id: &str,
        restamp_replies: bool,
    ) -> Option<RemovedMessage> {
        let mut guard = self.active.lock().ok()?;
        let active = guard.as_mut()?;
        if &active.room_id != room_id {
            return None;
        }
        let offset = active
            .messages
            .iter()
            .position(|message| message.event_id.as_deref() == Some(event_id))?;
        let message = active.messages.remove(offset);
        let mut replies = Vec::new();
        if restamp_replies {
            let deleted = HashSet::from([event_id.to_owned()]);
            for (index, reply) in active.messages.iter_mut().enumerate() {
                if stamp_deleted_reply(reply, &deleted) {
                    replies.push((active.prepended.saturating_add(index), reply.clone()));
                }
            }
        }
        Some(RemovedMessage {
            timeline_tx: active.timeline_tx.clone(),
            offset,
            row: active.prepended.saturating_add(offset),
            message,
            replies,
        })
    }

    async fn patch_queued_send(&self, room_id: &RoomId, local_id: &str, resend: bool) -> Result<()> {
        let prepared = {
            let Ok(mut guard) = self.active.lock() else {
                return Err(unavailable("the demo timeline"));
            };
            let Some(active) = guard.as_mut() else {
                return Err(unavailable("the demo timeline"));
            };
            if &active.room_id != room_id {
                return Err(unavailable("the demo timeline"));
            }
            let index = active
                .messages
                .iter()
                .position(|m| m.local_id.as_deref() == Some(local_id))
                .ok_or_else(|| AppError::Other(format!("no queued send for {local_id}")))?;
            let patch = if resend {
                let Some(message) = active.messages.get_mut(index) else {
                    return Err(unavailable("the demo timeline"));
                };
                mark_sent(message);
                TimelinePatch::Set {
                    index,
                    message: message.clone(),
                }
            } else {
                active.messages.remove(index);
                TimelinePatch::Remove { index }
            };
            self.settle_own_send(room_id, local_id, resend);
            (active.timeline_tx.clone(), patch)
        };
        let (timeline_tx, patch) = prepared;
        send_patch(&timeline_tx, patch).await;
        Ok(())
    }

    async fn append_own_message(
        &self,
        room_id: &RoomId,
        body: &str,
        emotes: &[Emote],
        in_reply_to: Option<&str>,
    ) {
        let prepared = {
            let Ok(mut guard) = self.active.lock() else {
                return;
            };
            let Some(active) = guard.as_mut() else {
                return;
            };
            if &active.room_id != room_id {
                return;
            }

            let reply = in_reply_to.and_then(|event_id| self.reply_in_room(active, event_id));
            let message = data::own_message(
                self.sent.fetch_add(1, Ordering::Relaxed),
                body,
                emotes,
                reply,
                self.send_state_in(room_id),
            );
            self.keep_own_send(room_id, &message);
            if !active.live {
                return;
            }
            active.messages.push(message.clone());
            (active.timeline_tx.clone(), message)
        };
        let (timeline_tx, message) = prepared;

        let seen = receipts::scenario().member_sees_sends && message.event_id.is_some();
        let unique_id = message.unique_id.clone();
        send_patch(&timeline_tx, TimelinePatch::PushBack(message)).await;
        if seen {
            spawn_seen(Arc::clone(&self.active), unique_id);
        }
    }

    async fn append_own_poll(&self, room_id: &RoomId, draft: &PollDraft) {
        let prepared = {
            let Ok(mut guard) = self.active.lock() else {
                return;
            };
            let Some(active) = guard.as_mut() else {
                return;
            };
            if &active.room_id != room_id {
                return;
            }
            let message = data::own_poll(
                self.sent.fetch_add(1, Ordering::Relaxed),
                draft,
                self.send_state_in(room_id),
            );
            self.keep_own_send(room_id, &message);
            if !active.live {
                return;
            }
            active.messages.push(message.clone());
            (active.timeline_tx.clone(), message)
        };
        let (timeline_tx, message) = prepared;
        send_patch(&timeline_tx, TimelinePatch::PushBack(message)).await;
    }

    async fn append_own_sticker(
        &self,
        room_id: &RoomId,
        image: &PackImage,
        in_reply_to: Option<&str>,
    ) {
        let prepared = {
            let Ok(mut guard) = self.active.lock() else {
                return;
            };
            let Some(active) = guard.as_mut() else {
                return;
            };
            if &active.room_id != room_id {
                return;
            }

            let reply = in_reply_to.and_then(|event_id| self.reply_in_room(active, event_id));
            let message =
                data::own_sticker(self.sent.fetch_add(1, Ordering::Relaxed), image, reply);
            self.keep_own_send(room_id, &message);
            if !active.live {
                return;
            }
            active.messages.push(message.clone());
            (active.timeline_tx.clone(), message)
        };
        let (timeline_tx, message) = prepared;

        send_patch(&timeline_tx, TimelinePatch::PushBack(message)).await;
    }

    async fn post_own_row(&self, room_id: &RoomId, message: TimelineMessage) {
        self.keep_own_send(room_id, &message);
        let timeline_tx = {
            let Ok(mut guard) = self.active.lock() else {
                return;
            };
            let Some(active) = guard
                .as_mut()
                .filter(|active| &active.room_id == room_id && active.live)
            else {
                return;
            };
            active.messages.push(message.clone());
            active.timeline_tx.clone()
        };
        send_patch(&timeline_tx, TimelinePatch::PushBack(message)).await;
    }

    fn timeline_sender(&self, room_id: &RoomId) -> Option<mpsc::Sender<TimelineUpdate>> {
        let guard = self.active.lock().ok()?;
        let active = guard.as_ref()?;
        (&active.room_id == room_id).then(|| active.timeline_tx.clone())
    }

    async fn append_own_attachment(
        &self,
        room_id: &RoomId,
        attachment: &OutgoingAttachment,
        opening: SendState,
    ) -> Option<(usize, TimelineMessage)> {
        let prepared = {
            let Ok(mut guard) = self.active.lock() else {
                return None;
            };
            let active = guard.as_mut()?;
            if &active.room_id != room_id {
                return None;
            }

            let reply = attachment
                .reply_to
                .as_deref()
                .and_then(|event_id| self.reply_in_room(active, event_id));
            let settled =
                data::own_attachment(self.sent.fetch_add(1, Ordering::Relaxed), attachment, reply);
            remember_sent_media(&settled, attachment);
            self.keep_own_send(room_id, &settled);
            if !active.live {
                return None;
            }
            active.messages.push(settled.clone());
            let index = active.messages.len() - 1;
            (active.timeline_tx.clone(), index, settled)
        };
        let (timeline_tx, index, settled) = prepared;

        let mut opening_echo = settled.clone();
        opening_echo.send_state = opening;
        send_patch(&timeline_tx, TimelinePatch::PushBack(opening_echo)).await;
        Some((index, settled))
    }

    async fn toggle_reaction(&self, event_id: &str, key: &str) {
        if reactions::scenario().toggle_has_no_echo {
            tracing::debug!(event_id, "demo: swallowing a reaction toggle");
            return;
        }
        let prepared = {
            let Ok(mut guard) = self.active.lock() else {
                return;
            };
            let Some(active) = guard.as_mut() else {
                return;
            };
            let Some(offset) = active
                .messages
                .iter()
                .position(|message| message.event_id.as_deref() == Some(event_id))
            else {
                return;
            };
            let row = active.prepended.saturating_add(offset);
            let Some(message) = active.messages.get_mut(offset) else {
                return;
            };
            let own = reactions::own_send(message, key);
            if !powers::messages().allows_toggle(own) {
                tracing::info!(
                    event_id,
                    ?own,
                    "demo: the power levels refuse this reaction toggle"
                );
                return;
            }
            reactions::toggle(message, key, data::own_user());
            (active.timeline_tx.clone(), row, message.clone())
        };
        let (timeline_tx, index, message) = prepared;

        send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
    }

    async fn run_command(
        &self,
        command: TimelineCommand,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) -> Option<PaginationDirection> {
        match command {
            TimelineCommand::PaginateBackwards => return Some(PaginationDirection::Backwards),
            TimelineCommand::PaginateForwards => return Some(PaginationDirection::Forwards),
            TimelineCommand::MarkRead => {}
            TimelineCommand::JumpTo(event_id) => {
                let target = self.loaded_row_of(&event_id);
                drop(
                    timeline_tx
                        .send(TimelineUpdate::JumpOutcome { event_id, target })
                        .await,
                );
            }
            TimelineCommand::ToggleReaction { event_id, key, .. } => {
                self.toggle_reaction(&event_id, &key).await;
            }
            TimelineCommand::VotePoll {
                event_id,
                answer_id,
            } => self.vote_poll(&event_id, &answer_id, timeline_tx).await,
            TimelineCommand::EndPoll { event_id } => self.end_poll(&event_id, timeline_tx).await,
            TimelineCommand::EditPoll { event_id, draft } => {
                self.edit_poll(&event_id, &draft, timeline_tx).await;
            }
            TimelineCommand::LocateAudio { request, lookup } => {
                let track = self.locate_audio(&lookup);
                drop(
                    timeline_tx
                        .send(TimelineUpdate::AudioLocated {
                            request,
                            track: track.map(Box::new),
                        })
                        .await,
                );
            }
            TimelineCommand::LocateSource { request, event_id } => {
                let source = self.locate_source(&event_id);
                drop(
                    timeline_tx
                        .send(TimelineUpdate::SourceLocated {
                            request,
                            source: source.map(Box::new),
                        })
                        .await,
                );
            }
            TimelineCommand::LocateReaders { request, event_id } => {
                let readers = self.locate_readers(&event_id);
                drop(
                    timeline_tx
                        .send(TimelineUpdate::ReadersLocated {
                            request,
                            readers: readers.map(Box::new),
                        })
                        .await,
                );
            }
        }
        None
    }

    fn locate_readers(&self, event_id: &str) -> Option<MessageReaders> {
        let guard = self.active.lock().ok()?;
        let active = guard.as_ref()?;
        receipts::readers_of(&active.messages, &active.receipts, event_id)
    }

    fn locate_source(&self, event_id: &str) -> Option<EventSource> {
        if message_menu::scenario().source_unavailable {
            return None;
        }
        let guard = self.active.lock().ok()?;
        let active = guard.as_ref()?;
        let message = active
            .messages
            .iter()
            .find(|message| message.event_id.as_deref() == Some(event_id))?;
        source::event_source(&active.room_id, message)
    }

    fn patch_poll(
        &self,
        event_id: &str,
        change: impl FnOnce(&mut TimelineMessage) -> bool,
    ) -> Option<(mpsc::Sender<TimelineUpdate>, usize, TimelineMessage)> {
        let mut guard = self.active.lock().ok()?;
        let active = guard.as_mut()?;
        let offset = active
            .messages
            .iter()
            .position(|message| message.event_id.as_deref() == Some(event_id))?;
        let row = active.prepended.saturating_add(offset);
        let message = active.messages.get_mut(offset)?;
        change(message).then(|| (active.timeline_tx.clone(), row, message.clone()))
    }

    async fn vote_poll(
        &self,
        event_id: &str,
        answer_id: &str,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) {
        if !powers::polls().vote {
            refuse_poll_action(timeline_tx, PollAction::Vote).await;
            return;
        }
        let mut previous = None;
        let Some((timeline_tx, index, message)) = self.patch_poll(event_id, |message| {
            previous = polls::cast(message, answer_id);
            previous.is_some()
        }) else {
            tracing::debug!(event_id, "demo: a vote that changes nothing");
            return;
        };
        send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        if polls::scenario().votes_fail
            && let Some(previous) = previous
        {
            spawn_poll_refusal(
                Arc::clone(&self.active),
                timeline_tx,
                event_id.to_owned(),
                Some(polls::Undo::Selection(previous)),
                PollAction::Vote,
            );
        }
    }

    async fn end_poll(&self, event_id: &str, timeline_tx: &mpsc::Sender<TimelineUpdate>) {
        if !powers::polls().end {
            refuse_poll_action(timeline_tx, PollAction::End).await;
            return;
        }
        let mut editable = false;
        let Some((timeline_tx, index, message)) = self.patch_poll(event_id, |message| {
            editable = message.body.poll().is_some_and(|poll| poll.editable);
            polls::end_own(message)
        }) else {
            tracing::debug!(event_id, "demo: only an own open poll can be ended");
            return;
        };
        send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        if polls::scenario().ends_fail {
            spawn_poll_refusal(
                Arc::clone(&self.active),
                timeline_tx,
                event_id.to_owned(),
                Some(polls::Undo::End { editable }),
                PollAction::End,
            );
        }
    }

    fn active_room_id(&self) -> Option<RoomId> {
        self.active
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|active| active.room_id.clone()))
    }

    async fn edit_poll(
        &self,
        event_id: &str,
        draft: &PollDraft,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) {
        if !powers::polls().start {
            refuse_poll_action(timeline_tx, PollAction::Edit).await;
            return;
        }
        let measured = self.active_room_id().map(|room_id| {
            with_room_facts(&room_id, |facts| {
                draft_size::poll_edit(draft, event_id, facts)
            })
        });
        if let Some(Err(oversize)) = measured.map(EventSize::checked) {
            tracing::debug!(event_id, %oversize, "demo: the poll edit is over the size limit");
            refuse_poll_action(timeline_tx, PollAction::Edit).await;
            return;
        }
        let sequence = self.sent.fetch_add(1, Ordering::Relaxed);
        let mut revised = polls::Revised::Refused;
        let Some((timeline_tx, index, message)) = self.patch_poll(event_id, |message| {
            revised = polls::revise(message, draft, sequence);
            revised == polls::Revised::Applied
        }) else {
            if revised == polls::Revised::Refused {
                refuse_poll_action(timeline_tx, PollAction::Edit).await;
            }
            return;
        };
        send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        if polls::scenario().edits_fail {
            spawn_poll_refusal(
                Arc::clone(&self.active),
                timeline_tx,
                event_id.to_owned(),
                None,
                PollAction::Edit,
            );
        }
    }

    fn loaded_row_of(&self, event_id: &str) -> JumpTarget {
        let row = self.active.lock().ok().and_then(|guard| {
            let active = guard.as_ref()?;
            let index_in_window = active
                .messages
                .iter()
                .position(|message| message.event_id.as_deref() == Some(event_id))?;
            Some(active.prepended.saturating_add(index_in_window))
        });
        row.map_or(JumpTarget::NotLoaded, JumpTarget::Row)
    }

    async fn append_newer(
        &self,
        page: Vec<TimelineMessage>,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) {
        if page.is_empty() {
            return;
        }
        if let Ok(mut guard) = self.active.lock()
            && let Some(active) = guard.as_mut()
        {
            active.messages.extend(page.iter().cloned());
        }
        send_patch(timeline_tx, TimelinePatch::Append(page)).await;
    }

    fn locate_audio(&self, lookup: &AudioLookup) -> Option<AudioTrack> {
        let guard = self.active.lock().ok()?;
        let active = guard.as_ref()?;
        locate_audio(lookup, active.messages.iter().cloned())
    }

    async fn run_timeline(
        &self,
        room_id: &RoomId,
        focus: TimelineFocus,
        timeline_tx: mpsc::Sender<TimelineUpdate>,
        mut cmd_rx: mpsc::UnboundedReceiver<TimelineCommand>,
    ) -> Result<()> {
        let scenario = timeline::scenario();
        if scenario.resolving_unread && focus.opens_at_read_position() {
            drop(timeline_tx.send(TimelineUpdate::ResolvingUnread).await);
        }
        if scenario.unread_boundary_is_unresolved && focus.opens_at_read_position() {
            drop(timeline_tx.send(TimelineUpdate::UnreadUnresolved).await);
        }
        if scenario.reset_is_slow {
            sleep(timeline::SLOW_RESET_DELAY).await;
        }
        let Some(OpenedWindow {
            all,
            mut newer,
            messages,
            reset_messages,
        }) = self
            .open_window(room_id, &focus, scenario, &timeline_tx)
            .await?
        else {
            return Ok(());
        };

        let opening_page = TimelineUpdate::Pagination {
            direction: PaginationDirection::Backwards,
            outcome: PaginationOutcome::Completed {
                hit_end: !scenario.pagination_returns_history,
            },
        };
        drop(timeline_tx.send(opening_page).await);

        if let Some(target) = focus.target() {
            let target_row = self.loaded_row_of(target);
            drop(
                timeline_tx
                    .send(TimelineUpdate::JumpOutcome {
                        event_id: target.to_owned(),
                        target: target_row,
                    })
                    .await,
            );
        }

        spawn_scenario_tasks(
            &self.active,
            scenario,
            &focus,
            &timeline_tx,
            &messages,
            reset_messages,
        );
        spawn_poll_activity(&self.active, &timeline_tx);
        self.spawn_poll_revocation();
        self.spawn_power_change();

        let mut history = 0_u64;
        while let Some(command) = cmd_rx.recv().await {
            let Some(direction) = self.run_command(command, &timeline_tx).await else {
                continue;
            };
            let mut hit_end = true;
            if scenario.pagination_returns_history
                && matches!(direction, PaginationDirection::Backwards)
            {
                history += 1;
                hit_end = history > 2;
                let page = older_history(history, &messages);
                if let Ok(mut guard) = self.active.lock()
                    && let Some(active) = guard.as_mut()
                {
                    active.prepended = active.prepended.saturating_add(page.len());
                }
                let page = page.into_iter().map(TimelinePatch::PushFront).collect();
                send_patch(&timeline_tx, TimelinePatch::Batch(page)).await;
            }
            if matches!(direction, PaginationDirection::Forwards) {
                let page = newer_page(&all, &mut newer);
                hit_end = page.is_empty();
                self.append_newer(page, &timeline_tx).await;
            }
            let update = TimelineUpdate::Pagination {
                direction,
                outcome: PaginationOutcome::Completed { hit_end },
            };
            if timeline_tx.send(update).await.is_err() {
                break;
            }
        }

        Ok(())
    }

    fn emit_verification(&self, event: VerificationEvent) {
        let Ok(guard) = self.verification_tx.lock() else {
            return;
        };
        let Some(tx) = guard.as_ref() else {
            return;
        };
        if tx.send(event).is_err() {
            tracing::debug!("demo: verification listener is gone");
        }
    }

    async fn stop_verification(&self, reason: VerificationCancellation) -> Result<()> {
        if self.take_outgoing() != OutgoingFlow::None {
            self.flows.fetch_add(1, Ordering::Relaxed);
            verification::pause().await;
            let reason = match reason {
                VerificationCancellation::Declined => VerificationCancellation::Withdrawn,
                reason => reason,
            };
            self.emit_verification(VerificationEvent::Cancelled(reason));
            return Ok(());
        }
        let Some(demo) = verification::requested() else {
            return Err(unavailable("Verification"));
        };
        verification::pause().await;
        if demo.fails == verification::FailingStep::Reject {
            return Err(unavailable("Declining a verification"));
        }
        self.emit_verification(VerificationEvent::Cancelled(reason));
        Ok(())
    }

    async fn confirm_own_request(&self) -> Result<()> {
        verification::pause().await;
        let for_this_session = self
            .outgoing
            .lock()
            .is_ok_and(|outgoing| *outgoing == OutgoingFlow::Session);
        if for_this_session && encryption::scenario().fails {
            return Err(unavailable("Confirming a verification"));
        }
        self.emit_verification(VerificationEvent::Confirming);
        verification::pause().await;
        match self.take_outgoing() {
            OutgoingFlow::None => return Ok(()),
            OutgoingFlow::Session => {
                self.encryption.verified_by_another_session();
                tracing::debug!("demo: another session verified this one");
            }
            OutgoingFlow::Person(user_id) => {
                self.verified_people.verify(&user_id);
                tracing::debug!(%user_id, "demo: verified the person");
            }
        }
        self.emit_verification(VerificationEvent::Done);
        Ok(())
    }

    fn take_outgoing(&self) -> OutgoingFlow {
        self.outgoing
            .lock()
            .map(|mut outgoing| mem::take(&mut *outgoing))
            .unwrap_or_default()
    }

    fn outgoing_runs(&self) -> bool {
        self.outgoing
            .lock()
            .is_ok_and(|outgoing| *outgoing != OutgoingFlow::None)
    }

    fn begin_outgoing(&self, flow: OutgoingFlow) -> u64 {
        if let Ok(mut outgoing) = self.outgoing.lock() {
            *outgoing = flow;
        }
        self.sas_started.store(false, Ordering::Relaxed);
        self.flows.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
    }

    fn trust_of(&self, user_id: &str) -> IdentityTrust {
        if !user_info::has_identity(user_id) {
            return IdentityTrust::NoIdentity;
        }
        if self.verified_people.contains(user_id) {
            return IdentityTrust::Verified;
        }
        if let Some(trust) = self.identity.trust_of(user_id) {
            return trust;
        }
        if user_info::verified_from_the_start(user_id) {
            IdentityTrust::Verified
        } else {
            IdentityTrust::Unverified
        }
    }

    fn user_trust(&self, user_id: &str) -> UserTrust {
        let identity = self.trust_of(user_id);
        UserTrust::new(identity, user_info::devices(user_id, identity))
    }
}

#[async_trait]
impl EncryptionPort for DemoAuthed {
    async fn follow(&self, state_tx: mpsc::Sender<AccountEncryption>) -> Result<()> {
        encryption::pause_first_assessment().await;
        let mut revision = self.encryption.subscribe();
        let mut reported = None;
        loop {
            let state = self.encryption.state();
            if reported != Some(state) {
                if state_tx.send(state).await.is_err() {
                    return Ok(());
                }
                reported = Some(state);
            }
            if revision.changed().await.is_err() {
                return Ok(());
            }
        }
    }

    async fn set_up_identity(&self, auth: Option<&AuthAnswer>) -> Result<IdentityOutcome> {
        if self.encryption.has_identity() {
            return Ok(IdentityOutcome::Ready);
        }
        let attempt = self.encryption.count_setup_attempt();
        encryption::pause_setup().await;
        if encryption::identity_refused()
            && let Guarded::NeedsAuth(challenge) = self.account.guard(auth, false)
        {
            tracing::debug!(
                "demo: the homeserver wants a password for the first cross-signing keys"
            );
            return Ok(IdentityOutcome::NeedsAuthentication(challenge));
        }
        if encryption::first_setup_fails(attempt) {
            return Err(unavailable("Creating the cross-signing identity"));
        }
        tracing::debug!("demo: created the account's cross-signing identity");
        self.encryption.create_identity();
        Ok(IdentityOutcome::Ready)
    }

    async fn recover(&self, key: &RecoveryKey) -> Result<RecoveryAttempt> {
        encryption::pause_action().await;
        if encryption::scenario().fails {
            return Err(unavailable("Using a recovery key"));
        }
        if !encryption::opens(key) {
            tracing::debug!("demo: the recovery key does not open the secret storage");
            return Ok(RecoveryAttempt::WrongKey);
        }
        let attempt = encryption::unlock_attempt();
        self.encryption.unlocked(attempt);
        Ok(attempt)
    }

    async fn enable_recovery(&self) -> Result<RecoverySetup> {
        encryption::pause_action().await;
        if encryption::scenario().fails {
            return Err(unavailable("Setting up recovery"));
        }
        self.encryption.recovery_enabled();
        tracing::debug!("demo: set up recovery");
        Ok(RecoverySetup::Created(encryption::created_key()))
    }

    async fn change_recovery_key(&self) -> Result<RecoveryKey> {
        encryption::pause_action().await;
        if encryption::scenario().fails {
            return Err(unavailable("Changing the recovery key"));
        }
        tracing::debug!("demo: changed the recovery key");
        Ok(encryption::changed_key())
    }

    async fn reset_identity(&self) -> Result<Guarded> {
        encryption::pause_action().await;
        if encryption::scenario().fails {
            return Err(unavailable("Resetting the identity"));
        }
        tracing::debug!("demo: the homeserver asks who you are before the new identity");
        Ok(self.account.guard(None, account::scenario().oauth))
    }

    async fn finish_identity_reset(&self, auth: &AuthAnswer) -> Result<Guarded> {
        encryption::pause_action().await;
        let guarded = self.account.guard(Some(auth), account::scenario().oauth);
        if guarded == Guarded::Done {
            tracing::debug!("demo: published the account's new identity");
            self.encryption.identity_reset();
        }
        Ok(guarded)
    }

    async fn abandon_identity_reset(&self) {
        tracing::debug!("demo: abandoned the identity reset");
    }

    async fn export_room_keys(&self, path: &Path, _passphrase: &KeyPassphrase) -> Result<usize> {
        encryption::pause_action().await;
        if encryption::scenario().fails {
            return Err(unavailable("Exporting room keys"));
        }
        tracing::info!(path = %path.display(), "demo: exported the room keys without writing a file");
        Ok(encryption::EXPORTED_KEYS)
    }

    async fn import_room_keys(&self, path: &Path, passphrase: &KeyPassphrase) -> Result<KeyImport> {
        encryption::pause_action().await;
        if encryption::scenario().fails {
            return Err(unavailable("Importing room keys"));
        }
        tracing::info!(path = %path.display(), "demo: importing room keys");
        Ok(encryption::import_from(passphrase))
    }
}

#[async_trait]
impl AccountPort for DemoAuthed {
    async fn facts(&self) -> AccountFacts {
        account::pause().await;
        account::facts()
    }

    async fn follow_sessions(&self, sessions_tx: mpsc::Sender<OwnSessions>) -> Result<()> {
        let device_id = data::session().device_id;
        let mut revision = self.account.subscribe();
        loop {
            if sessions_tx
                .send(self.account.sessions(&device_id))
                .await
                .is_err()
            {
                return Ok(());
            }
            if revision.changed().await.is_err() {
                return Ok(());
            }
        }
    }

    async fn rename_session(&self, device_id: &str, name: &str) -> Result<()> {
        account::pause().await;
        if account::scenario().fails {
            return Err(unavailable("Renaming a session"));
        }
        tracing::debug!(%device_id, "demo: renamed the session");
        self.account
            .rename(&data::session().device_id, device_id, name);
        Ok(())
    }

    async fn sign_out_session(
        &self,
        device_id: &str,
        auth: Option<&AuthAnswer>,
    ) -> Result<Guarded> {
        account::pause().await;
        if account::scenario().fails {
            return Err(unavailable("Signing out a session"));
        }
        let guarded = self.account.guard(auth, false);
        if guarded == Guarded::Done {
            tracing::debug!(%device_id, "demo: signed the session out");
            self.account.remove(device_id);
        }
        Ok(guarded)
    }

    async fn change_password(
        &self,
        _password: &AccountPassword,
        sign_out_others: bool,
        auth: Option<&AuthAnswer>,
    ) -> Result<Guarded> {
        account::pause().await;
        if account::scenario().fails {
            return Err(unavailable("Changing the password"));
        }
        let guarded = self.account.guard(auth, false);
        if guarded != Guarded::Done {
            return Ok(guarded);
        }
        if account::scenario().weak_password {
            return Err(AppError::WeakPassword(
                account::WEAK_PASSWORD_REASON.to_owned(),
            ));
        }
        tracing::debug!(sign_out_others, "demo: changed the password");
        if sign_out_others {
            self.account.remove_all_others();
        }
        Ok(Guarded::Done)
    }

    async fn deactivate(&self, erase: bool, auth: Option<&AuthAnswer>) -> Result<Guarded> {
        account::pause().await;
        if account::scenario().fails {
            return Err(unavailable("Deactivating the account"));
        }
        let guarded = self.account.guard(auth, false);
        if guarded == Guarded::Done {
            tracing::info!(erase, "demo: deactivated the account");
        }
        Ok(guarded)
    }

    async fn page(&self, page: &AccountPage) -> Option<LauncherSafeUrl> {
        let url = account::page(page)?;
        if let AccountPage::SignOut(device_id) = page {
            let book = Arc::clone(&self.account);
            let device_id = device_id.clone();
            tokio::spawn(async move {
                account::signed_out_in_browser(&book, &device_id).await;
            });
        }
        Some(url)
    }

    async fn logout_risks(&self) -> LogoutRisks {
        self.account
            .logout_risks(self.encryption.state().recovery == Recovery::On)
    }
}

#[async_trait]
impl SyncPort for DemoAuthed {
    async fn start_sync(&self, on_sync: SyncSink, cancel: CancellationToken) -> SyncOutcome {
        if let Ok(mut sink) = self.sync_sink.lock() {
            *sink = Some(Arc::clone(&on_sync));
        }
        self.hold_back_late_invites();
        self.keep_stored_removals();
        calls::book().begin();
        let lists = current_lists(&self.lists);
        on_sync(SyncEvent::Removals(lists.removals().into()));
        on_sync(SyncEvent::Rooms(lists.rooms().into()));
        on_sync(SyncEvent::Invites(lists.invites().into()));
        on_sync(SyncEvent::Spaces(lists.spaces().into()));
        if sync::support() == SyncSupport::ClassicOnly {
            return sync::refuse_forever(&on_sync, &cancel).await;
        }
        on_sync(SyncEvent::Connected);
        self.spawn_invite_changes();
        self.spawn_live_removal();
        self.spawn_call_script();
        if !timeline::scenario().room_list_keeps_updating {
            cancel.cancelled().await;
            return SyncOutcome::Cancelled;
        }
        loop {
            tokio::select! {
                () = cancel.cancelled() => return SyncOutcome::Cancelled,
                () = sleep(timeline::ROOM_LIST_INTERVAL) => {
                    let lists = current_lists(&self.lists);
                    on_sync(SyncEvent::Removals(lists.removals().into()));
                    on_sync(SyncEvent::Rooms(lists.rooms().into()));
                    on_sync(SyncEvent::Invites(lists.invites().into()));
                }
            }
        }
    }

    fn set_selected_room(&self, room_id: Option<&RoomId>) {
        tracing::debug!(?room_id, "demo: the selected room needs no subscription");
    }
}

impl DemoAuthed {
    fn sync_sink(&self) -> Option<SyncSink> {
        self.sync_sink.lock().ok().and_then(|sink| sink.clone())
    }

    fn change_lists(&self, change: impl FnOnce(&mut RoomLists)) {
        if let Ok(mut lists) = self.lists.lock() {
            change(&mut lists);
        }
    }

    fn remember_join(&self, room_id: &RoomId) {
        self.change_lists(|lists| {
            if !lists.joined.contains(room_id) {
                lists.joined.push(room_id.clone());
            }
            lists.overrides.left.remove(room_id.as_ref());
        });
    }

    fn spawn_poll_revocation(&self) {
        if !polls::scenario().permissions_get_revoked {
            return;
        }
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(polls::REVOKED_AFTER).await;
            if polls::revoke() {
                sink(SyncEvent::Rooms(current_lists(&lists).rooms().into()));
            }
        });
    }

    fn spawn_power_change(&self) {
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(powers::SWITCH_AFTER).await;
            if powers::switch() {
                sink(SyncEvent::Rooms(current_lists(&lists).rooms().into()));
            }
        });
    }

    fn echo_join(&self) {
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(space_index::JOIN_ECHO_LAG).await;
            let lists = current_lists(&lists);
            sink(SyncEvent::Removals(lists.removals().into()));
            sink(SyncEvent::Rooms(lists.rooms().into()));
            sink(SyncEvent::Invites(lists.invites().into()));
            sink(SyncEvent::Spaces(lists.spaces().into()));
        });
    }

    fn echo_departure(&self) {
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(room_info::ECHO_LAG).await;
            let lists = current_lists(&lists);
            sink(SyncEvent::Removals(lists.removals().into()));
            sink(SyncEvent::Rooms(lists.rooms().into()));
            sink(SyncEvent::Invites(lists.invites().into()));
            sink(SyncEvent::Spaces(lists.spaces().into()));
        });
    }

    fn keep_stored_removals(&self) {
        if !departures::scenario().stored {
            return;
        }
        self.change_lists(|lists| {
            for stored in departures::STORED_REMOVALS {
                lists
                    .joined
                    .retain(|joined| joined.as_ref() != stored.room_id);
                lists.overrides.left.insert(stored.room_id.to_owned());
                lists.overrides.removed.insert(
                    stored.room_id.to_owned(),
                    data::Removed::ago(stored.kind, stored.by, stored.reason, stored.minutes_ago),
                );
            }
        });
    }

    fn spawn_live_removal(&self) {
        let Some(kind) = departures::scenario().live_removal() else {
            return;
        };
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(departures::KICK_AFTER).await;
            if let Ok(mut lists) = lists.lock() {
                let room_id = departures::KICKED_ROOM;
                lists.joined.retain(|joined| joined.as_ref() != room_id);
                lists.overrides.left.insert(room_id.to_owned());
                lists.overrides.forgotten.remove(room_id);
                lists.overrides.removed.insert(
                    room_id.to_owned(),
                    data::Removed::now(
                        kind,
                        departures::KICKED_BY,
                        Some(departures::KICKED_REASON),
                    ),
                );
            }
            tracing::info!(
                room_id = departures::KICKED_ROOM,
                ?kind,
                "demo: you were removed from a room"
            );
            let lists = current_lists(&lists);
            sink(SyncEvent::Removals(lists.removals().into()));
            sink(SyncEvent::Rooms(lists.rooms().into()));
        });
    }

    fn hold_back_late_invites(&self) {
        if invites::scenario().arrives_late {
            self.change_lists(|lists| {
                lists
                    .overrides
                    .held_back
                    .insert(invites::LATE_INVITE.to_owned());
            });
        }
        if notifications::scenario().invite {
            self.change_lists(|lists| {
                lists
                    .overrides
                    .held_back
                    .insert(notifications::LATE_INVITE.to_owned());
            });
        }
    }

    fn emit_rooms(&self) {
        if let Some(sink) = self.sync_sink() {
            sink(SyncEvent::Rooms(current_lists(&self.lists).rooms().into()));
        }
    }

    async fn arrive(
        &self,
        arrival: &notifications::Arrival,
        notices_tx: &mpsc::Sender<PushNotice>,
    ) -> bool {
        let Some(room) = current_lists(&self.lists)
            .rooms()
            .into_iter()
            .find(|room| room.id == arrival.room_id)
        else {
            tracing::debug!(room_id = %arrival.room_id, "demo: an arrival names no listed room");
            return true;
        };
        let sender = data::newest_other_sender(&arrival.room_id)
            .unwrap_or_else(|| NamedUser::unnamed(room.id.to_string()));
        let message = data::arrived_message(
            self.sent.fetch_add(1, Ordering::Relaxed),
            &sender,
            arrival.body.clone(),
        );
        if let Ok(mut arrived) = self.arrived.lock() {
            arrived
                .entry(arrival.room_id.clone())
                .or_default()
                .push(message);
        }
        let shown_sender = (!room.is_direct).then_some(sender);
        if arrival.notifies {
            tracing::debug!(room_id = %arrival.room_id, "demo: push rules notify an arrival");
            let notice = PushNotice {
                room_id: arrival.room_id.clone(),
                room_name: room.display_name.clone(),
                sent_at: data::arrived_at(),
                content: NoticeContent::Message {
                    sender: shown_sender.clone(),
                    kind: arrival.kind,
                    body: arrival.preview(),
                },
                tweaks: arrival.tweaks,
            };
            if notices_tx.send(notice).await.is_err() {
                return false;
            }
        }
        sleep(notifications::ROOM_ECHO_LAG).await;
        let arrived = data::Arrived::now(
            shown_sender,
            arrival.kind,
            arrival.preview(),
            arrival.tweaks.highlight,
        );
        self.change_lists(|lists| {
            lists.overrides.read.remove(arrival.room_id.as_ref());
            lists
                .overrides
                .arrivals
                .insert(arrival.room_id.to_string(), arrived);
        });
        self.emit_rooms();
        true
    }

    fn read_elsewhere(&self, room_id: &RoomId) {
        tracing::debug!(%room_id, "demo: a room is read on another device");
        self.change_lists(|lists| {
            lists.overrides.read.insert(room_id.to_string());
        });
        self.emit_rooms();
    }

    async fn release_notified_invite(&self, notices_tx: &mpsc::Sender<PushNotice>) -> bool {
        self.change_lists(|lists| {
            lists.overrides.held_back.remove(notifications::LATE_INVITE);
        });
        let lists = current_lists(&self.lists);
        let Some(invite) = lists
            .invites()
            .into_iter()
            .find(|invite| invite.id.as_ref() == notifications::LATE_INVITE)
        else {
            return true;
        };
        let notice = PushNotice {
            room_id: invite.id.clone(),
            room_name: invite.name.clone(),
            sent_at: data::arrived_at(),
            content: NoticeContent::Invite {
                inviter: invite.inviter.clone(),
            },
            tweaks: PushTweaks {
                highlight: false,
                sound: true,
            },
        };
        tracing::debug!(room_id = %invite.id, "demo: push rules notify an invite");
        if notices_tx.send(notice).await.is_err() {
            return false;
        }
        sleep(notifications::ROOM_ECHO_LAG).await;
        if let Some(sink) = self.sync_sink() {
            sink(SyncEvent::Invites(lists.invites().into()));
        }
        true
    }

    fn spawn_invite_changes(&self) {
        let demo = invites::scenario();
        if demo.arrives_late {
            self.spawn_invite_change(invites::LATE_ARRIVAL, |overrides| {
                overrides.held_back.remove(invites::LATE_INVITE);
            });
        }
        if demo.gets_revoked {
            self.spawn_invite_change(invites::REVOKED_AFTER, |overrides| {
                overrides
                    .withdrawn
                    .insert(invites::REVOKED_INVITE.to_owned());
            });
        }
    }

    fn leave_call(&self, room_id: &RoomId) {
        calls::book().leave_own(room_id.as_ref());
        tracing::info!(%room_id, "demo: you left the call");
        self.echo_rooms();
    }

    fn script_answer(&self, room_id: &RoomId, ring: &RingId) {
        let shaped = calls::scenario();
        let Some(partner) = calls::partner_of(room_id.as_ref()) else {
            return;
        };
        if !(shaped.answers || shaped.declines) {
            return;
        }
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        let room_id = room_id.clone();
        let ring = ring.clone();
        tokio::spawn(async move {
            sleep(calls::ANSWER_AFTER).await;
            if shaped.answers {
                tracing::info!(%room_id, "demo: the call is answered");
                calls::answer_in(room_id.as_ref());
                sink(SyncEvent::Rooms(current_lists(&lists).rooms().into()));
                for key in calls::book().keys_from_others(room_id.as_ref()) {
                    calls::book().signal(key).await;
                }
            } else {
                tracing::info!(%room_id, "demo: the call is declined by the callee");
                calls::book()
                    .signal(CallSignal::Declined {
                        room_id,
                        ring,
                        by: UserId::new(partner),
                    })
                    .await;
            }
        });
    }

    fn spawn_call_script(&self) {
        let Some(sink) = self.sync_sink() else {
            return;
        };
        for scripted in calls::script() {
            let sink = Arc::clone(&sink);
            let lists = Arc::clone(&self.lists);
            tokio::spawn(async move {
                sleep(scripted.after).await;
                for step in (scripted.change)(calls::book()) {
                    match step {
                        Step::Rooms => {
                            tracing::debug!("demo: a call changed");
                            sink(SyncEvent::Rooms(current_lists(&lists).rooms().into()));
                        }
                        Step::Signal(signal) => calls::book().signal(signal).await,
                    }
                }
            });
        }
    }

    fn spawn_invite_change(
        &self,
        after: Duration,
        change: impl FnOnce(&mut data::RoomOverrides) + Send + 'static,
    ) {
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(after).await;
            if let Ok(mut lists) = lists.lock() {
                change(&mut lists.overrides);
            }
            tracing::debug!("demo: the invites changed");
            sink(SyncEvent::Invites(current_lists(&lists).invites().into()));
        });
    }

    fn person(&self, room_id: &RoomId, user_id: &UserId) -> UserProfile {
        let mut profile = data::profile(room_id, user_id, user_info::avatar);
        profile.direct_room = self.chat_with(user_id);
        profile.ignored = self.is_ignored(user_id);
        user_info::shape(&mut profile);
        if !profile.is_self {
            profile.trust = self.trust_of(user_id);
        }
        if let Some(membership) = self.membership_override(room_id, user_id) {
            profile.membership = membership;
        }
        profile.powers = user_info::powers(data::role_of(room_id, data::own_user()), &profile);
        profile
    }

    fn membership_override(&self, room_id: &RoomId, user_id: &str) -> Option<RoomMembership> {
        let memberships = self.memberships.lock().ok()?;
        memberships
            .get(&(room_id.clone(), user_id.to_owned()))
            .copied()
    }

    fn chat_with(&self, user_id: &str) -> Option<RoomId> {
        data::direct_room_with(user_id).or_else(|| {
            current_lists(&self.lists)
                .overrides
                .started_chats
                .iter()
                .find(|(_, person)| person.as_str() == user_id)
                .map(|(room_id, _)| RoomId::new(room_id))
        })
    }

    fn echo_rooms(&self) {
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(room_info::ECHO_LAG).await;
            sink(SyncEvent::Rooms(current_lists(&lists).rooms().into()));
        });
    }

    fn created(&self, room_id: &RoomId) -> Option<data::Created> {
        current_lists(&self.lists)
            .overrides
            .created
            .into_iter()
            .find(|created| created.id == room_id.as_ref())
    }

    fn address_taken(&self, wanted: &str) -> bool {
        let created = current_lists(&self.lists).overrides.created;
        data::alias_localparts().any(|taken| taken == wanted)
            || created.iter().any(|created| {
                created.alias.as_deref().is_some_and(|alias| {
                    alias
                        .strip_prefix('#')
                        .and_then(|alias| alias.split_once(':'))
                        .is_some_and(|(taken, _)| taken == wanted)
                })
            })
    }

    fn list_child(&self, parent: &RoomId, child: &RoomId) {
        self.change_lists(|lists| {
            lists
                .overrides
                .created_children
                .entry(parent.to_string())
                .or_default()
                .push(child.to_string());
        });
    }

    fn list_child_later(&self, parent: RoomId, child: RoomId) {
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(create_room::PARENT_LAG).await;
            if let Ok(mut lists) = lists.lock() {
                lists
                    .overrides
                    .created_children
                    .entry(parent.to_string())
                    .or_default()
                    .push(child.to_string());
            }
            tracing::debug!(%parent, %child, "demo: the parent space lists its new child");
            sink(SyncEvent::Spaces(current_lists(&lists).spaces().into()));
        });
    }

    fn invited_in(&self, room_id: &RoomId) -> Vec<String> {
        let Ok(memberships) = self.memberships.lock() else {
            return Vec::new();
        };
        let mut invited: Vec<String> = memberships
            .iter()
            .filter(|((room, _), membership)| {
                room == room_id && **membership == RoomMembership::Invited
            })
            .map(|((_, user_id), _)| user_id.clone())
            .collect();
        invited.sort();
        invited
    }

    fn session_roster(&self, room_id: &RoomId) -> Vec<RosterMember> {
        let mut roster = data::roster(room_id, room_info::member_avatar);
        if roster.is_empty() && self.created(room_id).is_some() {
            roster.push(data::named_member(
                data::own_user(),
                MemberRole::Admin,
                RosterSection::Joined,
                room_info::member_avatar,
            ));
        }
        for user_id in self.invited_in(room_id) {
            roster.retain(|member| member.user_id != user_id);
            roster.push(data::named_member(
                &user_id,
                MemberRole::Member,
                RosterSection::Invited,
                room_info::member_avatar,
            ));
        }
        sort_roster(&mut roster);
        roster
    }

    fn membership_in(&self, room_id: &RoomId, user_id: &str) -> RoomMembership {
        if let Some(membership) = self.membership_override(room_id, user_id) {
            return membership;
        }
        match self
            .session_roster(room_id)
            .into_iter()
            .find(|member| member.user_id == user_id)
            .map(|member| member.section)
        {
            Some(RosterSection::Joined) => RoomMembership::Joined,
            Some(RosterSection::Invited) => RoomMembership::Invited,
            None => RoomMembership::Outside,
        }
    }

    fn directory_person(&self, room_id: &RoomId, user_id: &str, name: Option<&str>) -> Person {
        Person {
            user_id: UserId::new(user_id),
            name: MemberName::new(name, false),
            avatar_mxc: Some(room_info::member_avatar(user_id)),
            membership: self.membership_in(room_id, user_id),
        }
    }
}

#[async_trait]
impl RoomCreationPort for DemoAuthed {
    async fn create_room(&self, blueprint: &RoomBlueprint) -> CreateResult {
        create_room::pause().await;
        let scenario = create_room::scenario();
        if scenario.refused {
            return Err(CreateFailure::Refused);
        }
        if scenario.fails {
            return Err(CreateFailure::Failed);
        }
        let own_server = server_part(data::own_user());
        let alias = match blueprint.access() {
            JoinAccess::Public(address) if self.address_taken(address.as_str()) => {
                return Err(CreateFailure::AddressTaken);
            }
            JoinAccess::Public(address) => Some(address.alias_on(own_server)),
            JoinAccess::InviteOnly | JoinAccess::SpaceMembers(_) => None,
        };
        let serial = self
            .creations
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let room_id = RoomId::new(data::created_room_id(serial));
        let child = match blueprint.parent() {
            None => ChildLink::NotAsked,
            Some(_) if scenario.child_fails => ChildLink::Failed,
            Some(_) => ChildLink::Added,
        };
        tracing::info!(%room_id, kind = ?blueprint.kind(), ?child, "demo: creating a room");
        if scenario.echo_lost {
            return Ok(CreatedRoom { room_id, child });
        }
        let created = data::Created {
            id: room_id.to_string(),
            space: blueprint.kind() == RoomKind::Space,
            name: blueprint.name().to_owned(),
            topic: blueprint.topic().map(str::to_owned),
            alias,
            encrypted: matches!(blueprint.shape(), Shape::Room { encrypted: true }),
            at_ms: data::now_ms(),
        };
        self.change_lists(|lists| lists.overrides.created.push(created));
        if let (ChildLink::Added, Some(parent)) = (child, blueprint.parent()) {
            if scenario.parent_late {
                self.list_child_later(parent.clone(), room_id.clone());
            } else {
                self.list_child(parent, &room_id);
            }
        }
        self.echo_join();
        Ok(CreatedRoom { room_id, child })
    }
}

#[async_trait]
impl InvitePeoplePort for DemoAuthed {
    async fn search_people(&self, room_id: &RoomId, term: &str) -> SearchResult {
        invite_people::pause().await;
        let term = term.trim();
        if invite_people::scenario().rate_limited {
            return Err(PeopleSearchFailure::RateLimited);
        }
        if invite_people::search_fails_now(term) {
            return Err(PeopleSearchFailure::Failed);
        }
        let own = data::own_user();
        let needle = term.to_lowercase();
        let mut matching: Vec<(String, String)> = data::directory_people()
            .into_iter()
            .filter(|(user_id, name)| {
                user_id != own
                    && (localpart(user_id).to_lowercase().contains(&needle)
                        || name.to_lowercase().contains(&needle))
            })
            .collect();
        let limit = if invite_people::scenario().limited {
            invite_people::LIMITED_RESULTS
        } else {
            usize::try_from(PEOPLE_SEARCH_LIMIT).unwrap_or(usize::MAX)
        };
        let limited = matching.len() > limit;
        matching.truncate(limit);
        let mut people: Vec<Person> = matching
            .iter()
            .map(|(user_id, name)| self.directory_person(room_id, user_id, Some(name)))
            .collect();
        if is_user_tag(term)
            && term != own
            && !people.iter().any(|person| person.user_id.as_ref() == term)
        {
            let name = data::person_name(term);
            people.insert(0, self.directory_person(room_id, term, name.as_deref()));
        }
        Ok(PeopleFound { people, limited })
    }

    async fn invite(
        &self,
        room_id: &RoomId,
        user_id: &UserId,
        reason: Option<&str>,
    ) -> InviteResult {
        invite_people::pause().await;
        let scenario = invite_people::scenario();
        if !invite_people::may_invite() {
            return Err(InviteFailure::Refused);
        }
        if scenario.invite_fails {
            return Err(InviteFailure::Failed);
        }
        if scenario.blocks_vera && user_id.as_ref() == invite_people::BLOCKED_USER {
            return Err(InviteFailure::Blocked);
        }
        if matches!(
            self.membership_in(room_id, user_id),
            RoomMembership::Joined | RoomMembership::Banned
        ) {
            return Err(InviteFailure::Refused);
        }
        tracing::info!(%room_id, %user_id, reason, "demo: inviting someone");
        if let Ok(mut memberships) = self.memberships.lock() {
            memberships.insert(
                (room_id.clone(), user_id.to_string()),
                RoomMembership::Invited,
            );
        }
        Ok(())
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        room_info::pause_avatars().await;
        media::fetch_member_avatars(mxcs)
    }
}

#[async_trait]
impl SpaceIndexPort for DemoAuthed {
    async fn hierarchy_page(&self, space_id: &RoomId, from: Option<&str>) -> Result<HierarchyPage> {
        space_index::pause().await;
        let offset = from.and_then(|token| token.parse().ok()).unwrap_or(0);
        if space_index::page_fails_now(space_id, offset) {
            return Err(unavailable("this page of the space index"));
        }
        let children = data::space_children(space_id);
        let next = offset.saturating_add(space_index::PAGE_SIZE);
        Ok(HierarchyPage {
            next: (next < children.len()).then(|| next.to_string()),
            children: children
                .into_iter()
                .skip(offset)
                .take(space_index::PAGE_SIZE)
                .collect(),
        })
    }

    async fn join(&self, room_id: &RoomId, via: &[String]) -> Result<()> {
        space_index::pause().await;
        if space_index::scenario().join_fails {
            return Err(unavailable("joining rooms"));
        }
        tracing::debug!(%room_id, ?via, "demo: joining a room from the space index");
        self.remember_join(room_id);
        self.echo_join();
        Ok(())
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        space_index::pause_avatars().await;
        media::fetch_unjoined_avatars(mxcs)
    }
}

#[async_trait]
impl InvitePort for DemoAuthed {
    async fn accept(&self, room_id: &RoomId) -> Result<()> {
        invites::pause().await;
        if invites::scenario().accept_fails {
            return Err(unavailable("accepting invites"));
        }
        tracing::debug!(%room_id, "demo: accepting an invite");
        if invites::scenario().echo_lost {
            return Ok(());
        }
        self.remember_join(room_id);
        self.echo_join();
        Ok(())
    }

    async fn decline(&self, room_id: &RoomId) -> Result<()> {
        invites::pause().await;
        if invites::scenario().decline_fails {
            return Err(unavailable("declining invites"));
        }
        tracing::debug!(%room_id, "demo: declining an invite");
        self.change_lists(|lists| {
            lists.overrides.declined.insert(room_id.to_string());
        });
        self.echo_join();
        Ok(())
    }
}

#[async_trait]
impl UpgradePort for DemoAuthed {
    async fn join_successor(&self, room_id: &RoomId) -> Result<()> {
        self.join_linked(room_id, data::successor_of(room_id)).await
    }

    async fn join_predecessor(&self, room_id: &RoomId) -> Result<()> {
        self.join_linked(room_id, data::predecessor_of(room_id))
            .await
    }
}

impl DemoAuthed {
    async fn join_linked(&self, room_id: &RoomId, linked: Option<RoomId>) -> Result<()> {
        upgrades::pause().await;
        if upgrades::scenario().join_fails {
            return Err(unavailable("joining across a room upgrade"));
        }
        let Some(linked) = linked else {
            return Err(AppError::Other(format!("no upgrade links {room_id}")));
        };
        tracing::debug!(%room_id, %linked, "demo: joining the other end of a room upgrade");
        if upgrades::scenario().echo_lost {
            return Ok(());
        }
        self.remember_join(&linked);
        self.echo_join();
        Ok(())
    }
}

#[async_trait]
impl RoomPreviewPort for DemoAuthed {
    async fn preview(&self, room: &RoomAddress, via: &[String]) -> PreviewResult {
        links::pause().await;
        tracing::debug!(%room, ?via, "demo: previewing a linked room");
        if links::preview_fails_now(room.as_str()) {
            return Err(PreviewFailure::Failed);
        }
        let room_id = match room {
            RoomAddress::Id(room_id) => room_id.clone(),
            RoomAddress::Alias(alias) => {
                data::alias_target(alias).ok_or(PreviewFailure::NoSuchRoom)?
            }
        };
        let membership = self.membership_of(&room_id);
        if links::scenario().unavailable && membership == PreviewMembership::Outsider {
            return Err(PreviewFailure::Unavailable);
        }
        let mut preview = data::room_card(&room_id).ok_or(PreviewFailure::Unavailable)?;
        preview.membership = membership;
        Ok(preview)
    }

    async fn join(
        &self,
        room: &RoomAddress,
        known: Option<&RoomId>,
        via: &[String],
    ) -> Result<RoomId> {
        links::pause().await;
        if links::scenario().join_fails {
            return Err(unavailable("joining linked rooms"));
        }
        let room_id = match room {
            RoomAddress::Id(room_id) => Some(room_id.clone()),
            RoomAddress::Alias(alias) => data::alias_target(alias),
        }
        .or_else(|| known.cloned())
        .filter(|room_id| data::room_card(room_id).is_some())
        .ok_or_else(|| AppError::Other(format!("no demo room has the address {room}")))?;
        tracing::debug!(%room_id, ?via, "demo: joining a linked room");
        if !links::scenario().echo_lost {
            self.remember_join(&room_id);
            self.echo_join();
        }
        Ok(room_id)
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        links::pause().await;
        media::fetch_unjoined_avatars(mxcs)
    }
}

impl DemoAuthed {
    fn membership_of(&self, room_id: &RoomId) -> PreviewMembership {
        let lists = current_lists(&self.lists);
        let joined = lists.rooms().iter().any(|room| room.id == *room_id)
            || lists
                .spaces()
                .iter()
                .any(|space| space.id == room_id.as_ref());
        if joined {
            return PreviewMembership::Joined;
        }
        if lists.invites().iter().any(|invite| invite.id == *room_id) {
            return PreviewMembership::Invited;
        }
        PreviewMembership::Outsider
    }
}

#[async_trait]
impl TypingPort for DemoAuthed {
    async fn follow_typing(
        &self,
        room_id: &RoomId,
        typists_tx: mpsc::Sender<Vec<NamedUser>>,
    ) -> Result<()> {
        let typists = typing::typists_in(room_id);
        if !typists.is_empty() {
            typing::type_in_the_room(typists, &typists_tx).await;
        }
        Ok(())
    }

    async fn notify_typing(&self, room_id: &RoomId, is_typing: bool) -> Result<()> {
        typing::pause_notice().await;
        if typing::scenario().notices_fail {
            return Err(unavailable("telling the room you are typing"));
        }
        self.typing_notices.notice(room_id, is_typing);
        Ok(())
    }
}

#[async_trait]
impl CallSignalPort for DemoAuthed {
    async fn follow_signals(&self, signals_tx: mpsc::Sender<CallSignal>) -> Result<()> {
        calls::book().follow(signals_tx.clone());
        signals_tx.closed().await;
        Ok(())
    }

    async fn decline(&self, room_id: &RoomId, ring: &RingId) -> Result<()> {
        calls::pause().await;
        if calls::scenario().decline_fails {
            return Err(unavailable("declining the call"));
        }
        tracing::info!(%room_id, %ring, "demo: the call is declined");
        Ok(())
    }

    async fn announce_call(&self, room_id: &RoomId, ring: &OutgoingRing) -> Result<RingId> {
        if calls::announcement_fails() {
            tracing::info!(%room_id, kind = ?ring.kind, "demo: the call announcement fails");
            return Err(unavailable("announcing the call"));
        }
        let id = RingId::new(format!("$demo-outgoing-{}", ring.sent_ms));
        tracing::info!(%room_id, kind = ?ring.kind, membership = ring.membership_event_id, "demo: the call is announced");
        self.script_answer(room_id, &id);
        Ok(id)
    }

    async fn send_key(
        &self,
        room_id: &RoomId,
        key: &OutgoingKey,
        to: &[MemberKey],
    ) -> KeySendReport {
        tracing::info!(%room_id, index = %key.index, to = to.len(), "demo: a call key is sent");
        KeySendReport {
            sent: to.to_vec(),
            withheld: Vec::new(),
        }
    }

    async fn device_standing(&self, members: &[MemberKey]) -> Vec<(MemberKey, DeviceStanding)> {
        let untrusted = calls::scenario().undecryptable;
        members
            .iter()
            .map(|member| {
                let standing = if untrusted && member.user == calls::UNTRUSTED {
                    DeviceStanding::NotCrossSigned
                } else {
                    DeviceStanding::CrossSigned
                };
                (member.clone(), standing)
            })
            .collect()
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        room_info::pause_avatars().await;
        media::fetch_member_avatars(mxcs)
    }
}

#[async_trait]
impl RtcMembershipPort for DemoAuthed {
    async fn own_transports(&self) -> Result<Vec<Transport>> {
        calls::pause().await;
        if calls::scenario().no_sfu {
            return Ok(Vec::new());
        }
        Ok(vec![calls::own_transport()])
    }

    async fn schedule_leave(&self, room_id: &RoomId, delay: Duration) -> ScheduleResult {
        if calls::scenario().delayed_unsupported {
            return Err(ScheduleFailure::Unsupported);
        }
        tracing::info!(%room_id, delay_ms = delay.as_millis(), "demo: the call's delayed leave is scheduled");
        Ok(DelayedLeave {
            room_id: room_id.clone(),
            delay_id: Secret::new("demo-delay"),
        })
    }

    async fn restart_leave(&self, _leave: &DelayedLeave) -> RestartResult {
        Ok(())
    }

    async fn send_leave(&self, leave: &DelayedLeave) -> Result<()> {
        self.leave_call(&leave.room_id);
        Ok(())
    }

    async fn clear_membership(&self, room_id: &RoomId) -> Result<()> {
        self.leave_call(room_id);
        Ok(())
    }

    async fn send_membership(
        &self,
        room_id: &RoomId,
        membership: &OwnMembership,
    ) -> Result<SentMembership> {
        calls::pause().await;
        let first = membership.created_ms.is_none();
        calls::book().own_join(room_id.as_ref(), membership.intent);
        tracing::info!(%room_id, first, "demo: you joined the call");
        self.echo_rooms();
        if first {
            spawn_others_keys(room_id);
        }
        Ok(SentMembership {
            event_id: format!("$demo-membership-{room_id}"),
            created_ms: membership.created_ms.unwrap_or_else(unix_now_ms),
        })
    }

    async fn sfu_grant(&self, _room_id: &RoomId, request: &GrantRequest) -> GrantResult {
        calls::pause().await;
        if calls::scenario().grant_fails {
            return Err(GrantFailure::Refused(GRANT_REFUSED));
        }
        let user = data::own_user();
        let [legacy, _, _] = spellings(user, &request.device, &request.member_id);
        Ok(SfuGrant {
            sfu: request.target.id.clone(),
            role: request.target.role,
            url: calls::DEMO_LIVEKIT.to_owned(),
            token: Secret::new("demo-token"),
            identity: legacy,
        })
    }
}

#[async_trait]
impl PresencePort for DemoAuthed {
    async fn announce(&self, availability: Availability, status: Option<&str>) -> BeaconResult {
        self.presence.announce(availability, status).await
    }

    async fn presence_of(&self, user_id: &UserId) -> LookupResult {
        self.presence.presence_of(user_id).await
    }
}

#[async_trait]
impl PushPort for DemoAuthed {
    async fn follow_notifications(&self, notices_tx: mpsc::Sender<PushNotice>) -> Result<()> {
        for timed in notifications::script() {
            sleep(timed.after).await;
            let following = match timed.step {
                notifications::Step::Arrive(arrival) => self.arrive(&arrival, &notices_tx).await,
                notifications::Step::ReadElsewhere(room_id) => {
                    self.read_elsewhere(&room_id);
                    true
                }
                notifications::Step::Invite => self.release_notified_invite(&notices_tx).await,
            };
            if !following {
                break;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl RoomInfoPort for DemoAuthed {
    async fn about(&self, room_id: &RoomId) -> Result<RoomAbout> {
        data::room_about(room_id)
            .or_else(|| self.created(room_id).as_ref().map(data::created_about))
            .ok_or_else(|| unavailable("this room's details"))
    }

    async fn roster(&self, room_id: &RoomId) -> Result<Vec<RosterMember>> {
        room_info::pause().await;
        if room_info::roster_fails_now(room_id) {
            return Err(unavailable("this room's member list"));
        }
        Ok(self.session_roster(room_id))
    }

    async fn readers(&self, room_id: &RoomId, user_ids: &[String]) -> Result<Vec<Reader>> {
        room_info::pause().await;
        Ok(data::readers(room_id, user_ids, room_info::member_avatar))
    }

    async fn set_notify(&self, room_id: &RoomId, mode: NotifyMode) -> Result<()> {
        room_info::pause().await;
        if room_info::scenario().notify_fails {
            return Err(unavailable("changing notifications"));
        }
        tracing::debug!(%room_id, ?mode, "demo: changing a room's notifications");
        if room_info::scenario().echo_is_lost {
            return Ok(());
        }
        self.change_lists(|lists| {
            lists.overrides.notify.insert(room_id.to_string(), mode);
        });
        self.echo_rooms();
        Ok(())
    }

    async fn mark_read(&self, room_id: &RoomId) -> Result<()> {
        room_info::pause().await;
        if room_info::scenario().read_fails {
            return Err(unavailable("marking rooms as read"));
        }
        tracing::debug!(%room_id, "demo: marking a room as read");
        self.change_lists(|lists| {
            lists.overrides.read.insert(room_id.to_string());
        });
        self.echo_rooms();
        Ok(())
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        room_info::pause_avatars().await;
        media::fetch_member_avatars(mxcs)
    }

    async fn room_link(&self, room_id: &RoomId) -> Result<String> {
        room_info::pause().await;
        if room_info::scenario().link_fails {
            return Err(unavailable("room links"));
        }
        data::room_link_of(room_id).ok_or_else(|| unavailable("this room's link"))
    }

    async fn event_link(&self, room_id: &RoomId, event_id: &str) -> Result<String> {
        message_menu::pause().await;
        if message_menu::scenario().link_fails {
            return Err(unavailable("message links"));
        }
        Ok(data::event_link(room_id, event_id))
    }
}

#[async_trait]
impl LeavePort for DemoAuthed {
    async fn leave(&self, room_id: &RoomId) -> Result<()> {
        room_info::pause().await;
        departures::pause().await;
        if room_info::scenario().leave_fails {
            return Err(unavailable("leaving rooms"));
        }
        if departures::scenario().room_stays && room_id.as_ref() == departures::STAYING_ROOM {
            return Err(unavailable("leaving this room"));
        }
        if account::scenario().forgets_rooms {
            tracing::info!(%room_id, "demo: leaving a room; the homeserver forgets it by itself");
        } else {
            tracing::info!(%room_id, "demo: leaving and forgetting a room");
        }
        self.change_lists(|lists| {
            lists.joined.retain(|joined| joined != room_id);
            lists.overrides.left.insert(room_id.to_string());
        });
        self.echo_departure();
        Ok(())
    }

    async fn forget(&self, room_id: &RoomId) -> Result<()> {
        departures::pause().await;
        if departures::scenario().forget_fails {
            return Err(unavailable("forgetting rooms"));
        }
        tracing::info!(%room_id, "demo: forgetting the room you were removed from");
        self.change_lists(|lists| {
            lists.overrides.forgotten.insert(room_id.to_string());
        });
        self.echo_departure();
        Ok(())
    }

    async fn admin_standing(&self, room_id: &RoomId) -> Result<AdminStanding> {
        departures::pause().await;
        tracing::debug!(%room_id, "demo: reading who else manages the room");
        if departures::scenario().sole_admin {
            return Ok(AdminStanding::OnlyAdmin);
        }
        Ok(AdminStanding::NotOnlyAdmin)
    }
}

#[async_trait]
impl UserInfoPort for DemoAuthed {
    async fn profile(&self, room_id: &RoomId, user_id: &UserId) -> Result<UserProfile> {
        if user_info::profile_fails_now(user_id) {
            return Err(unavailable("this profile"));
        }
        Ok(self.person(room_id, user_id))
    }

    async fn pronouns(&self, user_id: &UserId) -> Vec<String> {
        user_info::pause().await;
        data::pronouns(user_id)
    }

    async fn global_profile(&self, user_id: &UserId) -> Result<GlobalProfile> {
        user_info::pause().await;
        Ok(GlobalProfile {
            display_name: data::person_name(user_id),
            avatar_mxc: Some(user_info::avatar(user_id)),
            pronouns: data::pronouns(user_id),
        })
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        user_info::pause().await;
        media::fetch_member_avatars(mxcs)
    }

    async fn start_dm(&self, user_id: &UserId) -> Result<RoomId> {
        user_info::pause().await;
        if user_info::scenario().dm_fails {
            return Err(unavailable("starting chats"));
        }
        if let Some(existing) = self.chat_with(user_id) {
            return Ok(existing);
        }
        let room_id = RoomId::new(data::started_chat_id(user_id));
        tracing::debug!(%room_id, %user_id, "demo: starting a chat");
        if user_info::scenario().dm_echo_lost {
            return Ok(room_id);
        }
        self.change_lists(|lists| {
            lists
                .overrides
                .started_chats
                .insert(room_id.to_string(), user_id.to_string());
        });
        self.echo_rooms();
        Ok(room_id)
    }

    async fn moderate(
        &self,
        room_id: &RoomId,
        user_id: &UserId,
        action: Moderation,
        reason: Option<&str>,
    ) -> Result<()> {
        user_info::pause().await;
        if user_info::moderation_fails(action) {
            return Err(unavailable("moderating this room"));
        }
        if !self.person(room_id, user_id).offers(action) {
            return Err(unavailable("that moderation"));
        }
        tracing::info!(%room_id, %user_id, ?action, reason, "demo: moderating");
        if let Ok(mut memberships) = self.memberships.lock() {
            memberships.insert((room_id.clone(), user_id.to_string()), action.leaves());
        }
        Ok(())
    }

    async fn set_ignored(&self, user_id: &UserId, change: IgnoreChange) -> Result<()> {
        user_info::pause().await;
        if user_info::scenario().ignore_fails {
            return Err(unavailable("changing the ignore list"));
        }
        tracing::debug!(%user_id, ?change, "demo: changing the ignore list");
        if let Ok(mut ignored) = self.ignored.lock() {
            match change {
                IgnoreChange::Ignore => {
                    ignored.insert(user_id.to_string());
                }
                IgnoreChange::Unignore => {
                    ignored.remove(user_id.as_ref());
                }
            }
        }
        self.replay_live_window();
        Ok(())
    }

    async fn follow_trust(
        &self,
        user_id: &UserId,
        trust_tx: mpsc::Sender<UserTrust>,
    ) -> Result<()> {
        user_info::pause().await;
        let mut decisions = self.identity.subscribe();
        let mut verifications = self.verified_people.subscribe();
        let mut reported = None;
        loop {
            let trust = self.user_trust(user_id);
            if reported.as_ref() != Some(&trust) {
                if trust_tx.send(trust.clone()).await.is_err() {
                    return Ok(());
                }
                reported = Some(trust);
            }
            let changed = tokio::select! {
                changed = decisions.changed() => changed,
                changed = verifications.changed() => changed,
            };
            if changed.is_err() {
                return Ok(());
            }
        }
    }
}

#[async_trait]
impl IdentityPort for DemoAuthed {
    async fn follow_room(
        &self,
        room_id: &RoomId,
        alerts_tx: mpsc::Sender<Vec<RoomAlert>>,
    ) -> Result<()> {
        if !data::room_is_encrypted(room_id) {
            return Ok(());
        }
        identity::pause_room_alerts().await;
        let mut revision = self.identity.subscribe();
        let mut reported = None;
        loop {
            let alerts = self.identity.room_alerts(room_id);
            if reported.as_ref() != Some(&alerts) {
                if alerts_tx.send(alerts.clone()).await.is_err() {
                    return Ok(());
                }
                reported = Some(alerts);
            }
            if revision.changed().await.is_err() {
                return Ok(());
            }
        }
    }

    async fn follow_own(&self, own_tx: mpsc::Sender<OwnIdentity>) -> Result<()> {
        identity::pause_own_identity().await;
        let mut revision = self.identity.subscribe();
        let mut reported = None;
        loop {
            let own = self.identity.own_identity();
            if reported != Some(own) {
                if own_tx.send(own).await.is_err() {
                    return Ok(());
                }
                reported = Some(own);
            }
            if revision.changed().await.is_err() {
                return Ok(());
            }
        }
    }

    async fn pin_identity(&self, user_id: &UserId) -> Result<()> {
        identity::pause_action().await;
        if identity::scenario().acks_fail {
            return Err(unavailable("accepting a new identity"));
        }
        tracing::debug!(%user_id, "demo: accepting a new identity");
        self.identity.accept(user_id).map_err(AppError::Other)
    }

    async fn withdraw_verification(&self, user_id: &UserId) -> Result<()> {
        identity::pause_action().await;
        if identity::scenario().acks_fail {
            return Err(unavailable("withdrawing a verification"));
        }
        tracing::debug!(%user_id, "demo: withdrawing a verification");
        self.identity.withdraw(user_id).map_err(AppError::Other)
    }

    async fn acknowledge_own_reset(&self) -> Result<()> {
        identity::pause_action().await;
        if identity::scenario().acks_fail {
            return Err(unavailable("acknowledging an identity reset"));
        }
        tracing::debug!("demo: acknowledging the reset of your identity");
        self.identity.acknowledge_own_reset();
        Ok(())
    }
}

#[async_trait]
impl SpaceOrderPort for DemoAuthed {
    async fn set_space_order(&self, space_id: &RoomId, order: &str) -> Result<()> {
        tracing::debug!(%space_id, order, "demo: ignoring space order write");
        Ok(())
    }
}

fn reset_messages(messages: &[TimelineMessage]) -> Vec<TimelineMessage> {
    let messages = if reactions::scenario().reactions_arrive_late {
        reactions::strip_reactions(messages)
    } else {
        messages.to_vec()
    };
    let messages = if polls::scenario().tallies_arrive_late {
        polls::strip_tallies(&messages)
    } else {
        messages
    };
    if receipts::scenario().marks_arrive_late {
        receipts::strip_read_marks(&messages)
    } else {
        messages
    }
}

fn with_target_emotes(picked: &[Emote], target_html: Option<&str>) -> Vec<Emote> {
    let mut emotes = target_html.map(body_emoji::emotes_in).unwrap_or_default();
    for emote in picked {
        if !emotes.iter().any(|known| known.token == emote.token) {
            emotes.push(emote.clone());
        }
    }
    emotes
}

fn outgoing_send_state() -> SendState {
    if timeline::scenario().sends_fail {
        SendState::Failed
    } else if receipts::scenario().sends_stay_pending {
        SendState::Sending
    } else {
        SendState::Sent
    }
}

fn mark_sent(message: &mut TimelineMessage) {
    message.send_state = SendState::Sent;
    message.event_id = Some(message.unique_id.clone());
    message.local_id = None;
}

fn remember_sent_media(settled: &TimelineMessage, attachment: &OutgoingAttachment) {
    if let Some(event_id) = settled.event_id.as_deref()
        && !attachment.as_document
    {
        if let Some(preview) = attachment.picked.preview_path() {
            attachments::remember_preview(event_id, preview);
        }
        if attachment.picked.is_audio() {
            attachments::remember_sent_audio(event_id, &attachment.picked.path);
        }
    }
}

#[async_trait]
impl TimelinePort for DemoAuthed {
    async fn subscribe_timeline(
        &self,
        room_id: &RoomId,
        focus: TimelineFocus,
        timeline_tx: mpsc::Sender<TimelineUpdate>,
        cmd_rx: mpsc::UnboundedReceiver<TimelineCommand>,
        close: CancellationToken,
    ) -> Result<()> {
        let subscription = self.run_timeline(room_id, focus, timeline_tx, cmd_rx);
        close
            .run_until_cancelled(subscription)
            .await
            .unwrap_or(Ok(()))
    }

    async fn send_text(&self, room_id: &RoomId, body: &str, emotes: &[Emote]) -> Result<()> {
        powers::refuse_unless(powers::messages().send_messages, "sending messages")?;
        if timeline::scenario().sends_are_refused {
            return Err(unavailable("sending messages"));
        }
        with_room_facts(room_id, |facts| draft_size::message(body, emotes, facts)).checked()?;
        self.append_own_message(room_id, body, emotes, None).await;
        Ok(())
    }

    async fn send_reply(
        &self,
        room_id: &RoomId,
        body: &str,
        emotes: &[Emote],
        in_reply_to: &str,
    ) -> Result<()> {
        powers::refuse_unless(powers::messages().send_messages, "sending replies")?;
        if timeline::scenario().sends_are_refused {
            return Err(unavailable("sending replies"));
        }
        with_room_facts(room_id, |facts| {
            draft_size::reply(body, emotes, in_reply_to, facts)
        })
        .checked()?;
        self.append_own_message(room_id, body, emotes, Some(in_reply_to))
            .await;
        Ok(())
    }

    async fn edit_message(&self, room_id: &RoomId, edit: &MessageEdit) -> Result<()> {
        powers::refuse_unless(powers::messages().send_messages, "editing messages")?;
        if timeline::scenario().edits_are_refused {
            return Err(unavailable("editing messages"));
        }
        with_room_facts(room_id, |facts| draft_size::edit(edit, facts)).checked()?;
        let RevisedText {
            timeline_tx,
            row,
            previous,
            message,
        } = self.revise_own_text(room_id, edit).ok_or_else(|| {
            AppError::Other(
                "demo mode edits only an own text or caption in the open window".to_owned(),
            )
        })?;
        send_patch(
            &timeline_tx,
            TimelinePatch::Set {
                index: row,
                message,
            },
        )
        .await;
        if timeline::scenario().edits_fail && edit.target.event_id().is_some() {
            spawn_edit_refusal(
                Arc::clone(&self.active),
                timeline_tx,
                edit.clone(),
                previous,
            );
        }
        Ok(())
    }

    async fn send_poll(&self, room_id: &RoomId, draft: &PollDraft) -> Result<()> {
        powers::refuse_unless(powers::polls().start, "starting polls")?;
        if timeline::scenario().sends_are_refused {
            return Err(unavailable("sending polls"));
        }
        with_room_facts(room_id, |facts| draft_size::poll_start(draft, facts)).checked()?;
        self.append_own_poll(room_id, draft).await;
        Ok(())
    }

    async fn resend(&self, room_id: &RoomId, local_id: &str) -> Result<()> {
        if self.holds_sends_in(room_id) {
            tracing::debug!(%room_id, local_id, "demo: the retried send is held again");
            return Ok(());
        }
        self.patch_queued_send(room_id, local_id, true).await
    }

    async fn discard_send(&self, room_id: &RoomId, local_id: &str) -> Result<()> {
        self.patch_queued_send(room_id, local_id, false).await
    }

    async fn delete_message(&self, room_id: &RoomId, event_id: &str) -> Result<()> {
        message_menu::pause().await;
        if self
            .deleted
            .lock()
            .is_ok_and(|deleted| deleted.contains(event_id))
        {
            tracing::debug!(event_id, "demo: the message is already deleted");
            return Ok(());
        }
        let own = self
            .room_history(room_id)
            .iter()
            .find(|message| message.event_id.as_deref() == Some(event_id))
            .map(|message| message.is_own)
            .ok_or_else(|| AppError::Other(format!("{event_id} is not in {room_id}")))?;
        let permissions = powers::messages();
        let allowed = if own {
            permissions.delete_own
        } else {
            permissions.delete_others
        };
        if !allowed {
            return Err(unavailable("deleting this message"));
        }
        if message_menu::scenario().delete_refused {
            return Err(unavailable("deleting messages"));
        }
        tracing::debug!(%room_id, event_id, "demo: deleting a message");
        let fails = message_menu::scenario().delete_fails;
        if !fails && let Ok(mut deleted) = self.deleted.lock() {
            deleted.insert(event_id.to_owned());
        }
        let Some(removed) = self.remove_from_window(room_id, event_id, !fails) else {
            return Ok(());
        };
        let timeline_tx = removed.timeline_tx.clone();
        send_patch(&timeline_tx, TimelinePatch::Remove { index: removed.row }).await;
        for (index, message) in removed.replies.clone() {
            send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        }
        if fails {
            spawn_deletion_refusal(Arc::clone(&self.active), removed);
        } else {
            self.change_board(room_id, |_| {});
        }
        Ok(())
    }

    async fn send_attachment(
        &self,
        room_id: &RoomId,
        attachment: &OutgoingAttachment,
        abandon: CancellationToken,
    ) -> Result<AttachmentHandoff> {
        powers::refuse_unless(powers::messages().send_messages, "sending attachments")?;
        if attachments::scenario().refuses_size {
            return Err(AppError::AttachmentTooLarge {
                limit: attachments::upload_limit(),
            });
        }
        let content_type = if attachment.as_document {
            mime::APPLICATION_OCTET_STREAM.as_ref()
        } else {
            attachment.picked.mimetype.as_str()
        };
        let media = MediaDraft {
            attachment,
            content_type,
            thumbnail: false,
        };
        with_room_facts(room_id, |facts| draft_size::media(&media, facts)).checked()?;
        if attachments::scenario().send_fails {
            tokio::select! {
                biased;
                () = abandon.cancelled() => return Ok(AttachmentHandoff::Abandoned),
                () = attachments::pause_upload() => {}
            }
            return Err(unavailable("sending attachments"));
        }

        let total = attachment.picked.size;
        let uploads_slowly = attachments::scenario().upload_is_slow;
        let opening = if uploads_slowly {
            SendState::Uploading { sent: 0, total }
        } else {
            SendState::Sent
        };
        let Some((index, settled)) = self
            .append_own_attachment(room_id, attachment, opening)
            .await
        else {
            return Ok(AttachmentHandoff::Queued);
        };
        if uploads_slowly && let Some(timeline_tx) = self.timeline_sender(room_id) {
            spawn_upload_progress(timeline_tx, index, settled, total);
        }
        Ok(AttachmentHandoff::Queued)
    }
}

#[async_trait]
impl PinnedPort for DemoAuthed {
    async fn subscribe_pinned(
        &self,
        room_id: &RoomId,
        pinned_tx: mpsc::Sender<Vec<PinnedMessage>>,
    ) -> Result<()> {
        pinned::pause_arrival().await;
        let forward = self.forward_pins(room_id, &pinned_tx);
        let repinned = pinned::scenario()
            .repins
            .then(|| data::latest_pinnable(room_id))
            .flatten();
        match repinned {
            Some(newest) => {
                tokio::select! {
                    () = forward => {}
                    () = self.repin(room_id, &newest.event_id) => {}
                }
            }
            None => forward.await,
        }
        Ok(())
    }

    async fn change_pin(&self, room_id: &RoomId, event_id: &str, change: PinChange) -> Result<()> {
        message_menu::pause().await;
        if !powers::messages().pin {
            return Err(unavailable("pinning in this room"));
        }
        if message_menu::scenario().pin_fails {
            return Err(unavailable("changing pins"));
        }
        tracing::debug!(%room_id, event_id, ?change, "demo: changing a pin");
        self.change_board(room_id, |pins| match change {
            PinChange::Pin => {
                if !pins.iter().any(|pin| pin == event_id) {
                    pins.push(event_id.to_owned());
                }
            }
            PinChange::Unpin => pins.retain(|pin| pin != event_id),
        });
        Ok(())
    }
}

impl DemoAuthed {
    fn pin_board(&self, room_id: &RoomId) -> Option<watch::Receiver<Vec<String>>> {
        let mut boards = self.pin_boards.lock().ok()?;
        let board = boards.entry(room_id.clone()).or_insert_with(|| {
            let pins = data::pinned_messages(room_id)
                .into_iter()
                .map(|pin| pin.event_id)
                .collect();
            watch::channel(pins).0
        });
        Some(board.subscribe())
    }

    fn change_board(&self, room_id: &RoomId, change: impl FnOnce(&mut Vec<String>)) {
        drop(self.pin_board(room_id));
        if let Ok(boards) = self.pin_boards.lock()
            && let Some(board) = boards.get(room_id)
        {
            board.send_modify(change);
        }
    }

    fn pinned_from(&self, room_id: &RoomId, pins: &[String]) -> Vec<PinnedMessage> {
        let history = self.room_history(room_id);
        pins.iter()
            .filter_map(|pin| {
                history
                    .iter()
                    .rev()
                    .find(|message| message.event_id.as_deref() == Some(pin.as_str()))
            })
            .filter_map(data::pinned_message)
            .collect()
    }

    async fn forward_pins(&self, room_id: &RoomId, pinned_tx: &mpsc::Sender<Vec<PinnedMessage>>) {
        let Some(mut board) = self.pin_board(room_id) else {
            return;
        };
        loop {
            let pins = board.borrow_and_update().clone();
            if pinned_tx
                .send(self.pinned_from(room_id, &pins))
                .await
                .is_err()
            {
                return;
            }
            tokio::select! {
                changed = board.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
                () = pinned_tx.closed() => return,
            }
        }
    }

    async fn repin(&self, room_id: &RoomId, event_id: &str) {
        loop {
            sleep(pinned::REPIN_INTERVAL).await;
            self.change_board(room_id, |pins| {
                if pins.iter().any(|pin| pin == event_id) {
                    pins.retain(|pin| pin != event_id);
                } else {
                    pins.push(event_id.to_owned());
                }
            });
        }
    }
}

#[async_trait]
impl MediaPort for DemoAuthed {
    async fn download_media(
        &self,
        _room_id: &RoomId,
        event_id: &str,
        _rendition: MediaRendition,
    ) -> Result<Vec<u8>> {
        let path: PathBuf = media::DemoMediaCache
            .thumbnail_path(&media::content_of(event_id))
            .ok_or_else(|| AppError::Other(format!("no demo asset for event {event_id}")))?;
        Ok(fs::read(path)?)
    }

    async fn materialize_video(&self, _room_id: &RoomId, event_id: &str) -> Result<PathBuf> {
        videos::pause_download().await;
        media::video_asset_path(event_id)
            .ok_or_else(|| AppError::Other(format!("no demo video for event {event_id}")))
    }

    async fn materialize_audio(
        &self,
        _room_id: &RoomId,
        event_id: &str,
        need: WaveformNeed,
    ) -> Result<PathBuf> {
        audio::pause_download().await;
        let path = media::fetch_audio(event_id)
            .ok_or_else(|| AppError::Other(format!("no demo audio for event {event_id}")))?;
        if need == WaveformNeed::Compute {
            let owned = path.clone();
            let learned = spawn_blocking(move || video::probe_audio(&owned))
                .await
                .ok()
                .flatten()
                .and_then(|probe| probe.waveform);
            if let Some(waveform) = learned {
                media::remember_waveform(event_id, waveform);
            }
        }
        Ok(path)
    }
}

#[async_trait]
impl ImagePackPort for DemoAuthed {
    async fn catalog(&self, room_id: &RoomId) -> Result<PackCatalog> {
        let demo = stickers::scenario();
        stickers::pause_catalog().await;
        if demo.catalog_fails {
            return Err(unavailable("loading sticker packs"));
        }
        let packs = if demo.catalog_is_empty {
            Vec::new()
        } else {
            let shared = self
                .shared_packs
                .lock()
                .map(|shared| shared.clone())
                .unwrap_or_default();
            data::image_packs(room_id, &shared)
        };
        Ok(PackCatalog {
            packs,
            room_encrypted: demo.room_is_encrypted || data::room_is_encrypted(room_id),
        })
    }

    async fn prefetch(&self, mxcs: &[String]) -> usize {
        stickers::pause_prefetch().await;
        media::prefetch_pack_images(mxcs)
    }

    async fn send_sticker(
        &self,
        room_id: &RoomId,
        pack: &PackId,
        shortcode: &str,
        in_reply_to: Option<&str>,
    ) -> Result<()> {
        powers::refuse_unless(powers::messages().send_stickers, "sending stickers")?;
        if stickers::scenario().send_fails {
            return Err(unavailable("sending stickers"));
        }
        let image = data::sticker_image(pack, shortcode)
            .ok_or_else(|| AppError::Other(format!("no demo sticker {pack}/{shortcode}")))?;
        self.append_own_sticker(room_id, &image, in_reply_to).await;
        Ok(())
    }

    async fn set_global(&self, pack: &PackId, global: bool) -> Result<()> {
        if stickers::scenario().global_fails {
            return Err(unavailable("sharing image packs"));
        }
        if !data::is_room_pack(pack) {
            return Err(AppError::Other(format!("no demo room pack {pack}")));
        }
        if let Ok(mut shared) = self.shared_packs.lock() {
            shared.insert(pack.to_string(), global);
        }
        Ok(())
    }
}

#[async_trait]
impl VerificationPort for DemoAuthed {
    async fn listen_for_verification(
        &self,
        tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Result<()> {
        if let Ok(mut guard) = self.verification_tx.lock() {
            *guard = Some(tx);
        }

        if let Some(demo) = verification::requested() {
            verification::wait_for_request().await;
            self.sas_started.store(false, Ordering::Relaxed);
            self.emit_verification(verification::request(demo));

            if demo.times_out {
                verification::wait_for_request().await;
                self.emit_verification(VerificationEvent::Cancelled(
                    VerificationCancellation::TimedOut,
                ));
            }
        }

        pending::<()>().await;
        Ok(())
    }

    async fn verify_this_session(&self) -> Result<()> {
        encryption::pause_action().await;
        if encryption::scenario().fails {
            return Err(unavailable("Asking your other sessions"));
        }
        if !self.encryption.state().verified_sessions {
            return Err(AppError::Other(
                "no other session of this account can verify this one".to_owned(),
            ));
        }
        let Some(tx) = self
            .verification_tx
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
        else {
            return Err(AppError::Other(
                "verification requests are not followed".to_owned(),
            ));
        };
        let flow = self.begin_outgoing(OutgoingFlow::Session);
        tracing::debug!("demo: asked the account's other sessions to verify this one");
        self.emit_verification(VerificationEvent::RequestSent {
            to: data::own_user().to_owned(),
            is_self: true,
        });
        let flows = Arc::clone(&self.flows);
        let sas_started = Arc::clone(&self.sas_started);
        tokio::spawn(async move {
            encryption::pause_sas().await;
            if flows.load(Ordering::Relaxed) == flow {
                sas_started.store(true, Ordering::Relaxed);
                if tx.send(verification::emojis()).is_err() {
                    tracing::debug!("demo: verification listener is gone");
                }
            }
        });
        Ok(())
    }

    async fn verify_user(&self, user_id: &UserId) -> Result<()> {
        user_info::pause().await;
        if user_info::scenario().verify_fails {
            return Err(unavailable("Asking people to verify"));
        }
        let Some(chat) = self.chat_with(user_id) else {
            return Err(AppError::Other(
                "there is no direct chat with this person to send the request in".to_owned(),
            ));
        };
        if self.trust_of(user_id) != IdentityTrust::Unverified {
            return Err(AppError::Other(
                "this person cannot be verified now".to_owned(),
            ));
        }
        let Some(tx) = self
            .verification_tx
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
        else {
            return Err(AppError::Other(
                "verification requests are not followed".to_owned(),
            ));
        };
        let flow = self.begin_outgoing(OutgoingFlow::Person(user_id.to_string()));
        tracing::debug!(%user_id, room_id = %chat, "demo: asked the person to verify in your chat");
        let target = NamedUser::new(user_id.to_string(), data::names_in(&chat, []).of(user_id));
        let request =
            data::own_verification_request(self.sent.fetch_add(1, Ordering::Relaxed), target);
        self.post_own_row(&chat, request).await;
        self.emit_verification(VerificationEvent::RequestSent {
            to: user_id.to_string(),
            is_self: false,
        });
        let flows = Arc::clone(&self.flows);
        let sas_started = Arc::clone(&self.sas_started);
        let outgoing = Arc::clone(&self.outgoing);
        let answer = user_info::scenario().verify_answer;
        tokio::spawn(async move {
            let event = match answer {
                user_info::VerifyAnswer::Emoji => {
                    encryption::pause_sas().await;
                    verification::emojis()
                }
                user_info::VerifyAnswer::Declined => {
                    encryption::pause_sas().await;
                    VerificationEvent::Cancelled(VerificationCancellation::Remote)
                }
                user_info::VerifyAnswer::Unanswered => {
                    verification::wait_for_request().await;
                    VerificationEvent::Cancelled(VerificationCancellation::TimedOut)
                }
            };
            if flows.load(Ordering::Relaxed) != flow {
                return;
            }
            let ended = matches!(event, VerificationEvent::Cancelled(_));
            if ended && let Ok(mut outgoing) = outgoing.lock() {
                *outgoing = OutgoingFlow::None;
            }
            sas_started.store(
                matches!(event, VerificationEvent::Emojis(_)),
                Ordering::Relaxed,
            );
            if tx.send(event).is_err() {
                tracing::debug!("demo: verification listener is gone");
            }
        });
        Ok(())
    }

    async fn accept_verification(&self) -> Result<()> {
        let Some(demo) = verification::requested() else {
            return Err(unavailable("Verification"));
        };
        verification::pause().await;
        if demo.fails == verification::FailingStep::Accept {
            return Err(unavailable("Accepting a verification"));
        }
        self.sas_started.store(true, Ordering::Relaxed);
        self.emit_verification(verification::emojis());
        Ok(())
    }

    async fn confirm_verification(&self) -> Result<()> {
        if self.outgoing_runs() {
            return self.confirm_own_request().await;
        }
        let Some(demo) = verification::requested() else {
            return Err(unavailable("Verification"));
        };
        verification::pause().await;
        if demo.fails == verification::FailingStep::Confirm {
            return Err(unavailable("Confirming a verification"));
        }
        self.emit_verification(VerificationEvent::Confirming);
        verification::pause().await;
        self.emit_verification(VerificationEvent::Done);
        Ok(())
    }

    async fn reject_verification(&self) -> Result<()> {
        let reason = if self.sas_started.load(Ordering::Relaxed) {
            VerificationCancellation::Mismatch
        } else {
            VerificationCancellation::Declined
        };
        self.stop_verification(reason).await
    }

    async fn cancel_verification(&self) -> Result<()> {
        self.stop_verification(VerificationCancellation::Declined)
            .await
    }
}

#[async_trait]
impl SessionPort for DemoAuthed {
    async fn subscribe_session_changes(
        &self,
        _session_tx: mpsc::UnboundedSender<Session>,
    ) -> Result<()> {
        Ok(())
    }

    async fn fetch_user_avatar(&self) -> Result<Option<PathBuf>> {
        Ok(media::user_avatar_path())
    }

    async fn logout(&self) -> DeviceRemoval {
        logout::answer().await
    }

    async fn suspend(&self) {}

    async fn clear_store(&self) -> CleanupReport {
        CleanupReport::default()
    }
}

fn reply_info(messages: &[TimelineMessage], event_id: &str) -> Option<ReplyInfo> {
    messages
        .iter()
        .find(|message| message.event_id.as_deref() == Some(event_id))
        .map(|message| ReplyInfo {
            event_id: event_id.to_owned(),
            sender: data::sender_of(message),
            kind: message.body.preview_kind(),
            body: data::body_preview(&message.body),
        })
}

fn opening_window(
    all: &[TimelineMessage],
    focus: &TimelineFocus,
    scenario: timeline::Scenario,
) -> Result<Range<usize>> {
    match focus.target() {
        Some(target) => focused_window(all, target),
        None if scenario.window_is_short => Ok(newest(all, timeline::SHORT_WINDOW)),
        None => Ok(0..all.len()),
    }
}

fn window_messages(
    all: &[TimelineMessage],
    window: Range<usize>,
    focus: &TimelineFocus,
) -> Vec<TimelineMessage> {
    let mut messages = all.get(window).unwrap_or(all).to_vec();
    if matches!(focus, TimelineFocus::Latest) {
        clear_unread_divider(&mut messages);
    }
    messages
}

fn clear_unread_divider(messages: &mut [TimelineMessage]) {
    for message in messages {
        message.is_first_unread = false;
    }
}

fn newest(messages: &[TimelineMessage], count: usize) -> Range<usize> {
    messages.len().saturating_sub(count)..messages.len()
}

fn focused_window(messages: &[TimelineMessage], target: &str) -> Result<Range<usize>> {
    let index = messages
        .iter()
        .position(|message| message.event_id.as_deref() == Some(target))
        .ok_or(AppError::FocusedEventUnavailable)?;
    let start = index.saturating_sub(timeline::FOCUS_CONTEXT);
    let end = messages
        .len()
        .min(index.saturating_add(timeline::FOCUS_CONTEXT));
    Ok(start..end)
}

fn newer_page(all: &[TimelineMessage], newer: &mut usize) -> Vec<TimelineMessage> {
    let end = all.len().min(newer.saturating_add(timeline::NEWER_PAGE));
    let page = all.get(*newer..end).unwrap_or_default().to_vec();
    *newer = end;
    page
}

fn opening_patch(scenario: timeline::Scenario, messages: Vec<TimelineMessage>) -> TimelinePatch {
    let reset = TimelinePatch::Reset(messages);
    if scenario.reset_is_batched {
        return TimelinePatch::Batch(vec![reset]);
    }
    reset
}

fn spawn_late_reset(
    scenario: timeline::Scenario,
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    messages: Vec<TimelineMessage>,
) {
    tokio::spawn(async move {
        sleep(timeline::REPEATED_RESET_DELAY).await;
        let mut messages = messages;
        if scenario.repeat_drops_anchor {
            let keep = messages.len().saturating_sub(3);
            messages = messages.split_off(keep);
        }
        send_patch(&timeline_tx, opening_patch(scenario, messages)).await;
    });
}

fn spawn_row_churn(timeline_tx: mpsc::Sender<TimelineUpdate>, messages: Vec<TimelineMessage>) {
    tokio::spawn(async move {
        for round in 0..timeline::RESIZE_ROUNDS {
            sleep(timeline::RESIZE_INTERVAL).await;
            let Some(index) = messages.len().checked_sub(1) else {
                return;
            };
            let Some(message) = messages.get(index) else {
                return;
            };
            let mut message = message.clone();
            message.body = grown_body(&message.body, round);
            send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        }
    });
}

fn spawn_scenario_tasks(
    active: &SharedActiveRoom,
    scenario: timeline::Scenario,
    focus: &TimelineFocus,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
    messages: &[TimelineMessage],
    reset_messages: Vec<TimelineMessage>,
) {
    if focus.is_live() && scenario.reset_repeats {
        spawn_late_reset(scenario, timeline_tx.clone(), reset_messages);
    }
    if scenario.rows_keep_resizing {
        spawn_row_churn(timeline_tx.clone(), messages.to_vec());
    }
    if scenario.message_arrives_late {
        spawn_late_append(Arc::clone(active), timeline_tx.clone());
    }
    if reactions::scenario().reactions_arrive_late {
        spawn_late_sets(
            timeline_tx.clone(),
            messages.to_vec(),
            reactions::reacted_indices(messages),
            reactions::LATE_INTERVAL,
        );
    }
    if polls::scenario().tallies_arrive_late {
        spawn_late_sets(
            timeline_tx.clone(),
            messages.to_vec(),
            polls::poll_indices(messages),
            polls::LATE_INTERVAL,
        );
    }
    if receipts::scenario().marks_arrive_late {
        spawn_late_sets(
            timeline_tx.clone(),
            messages.to_vec(),
            receipts::read_indices(messages),
            receipts::LATE_INTERVAL,
        );
    }
}

fn spawn_seen(active: SharedActiveRoom, unique_id: String) {
    tokio::spawn(async move {
        sleep(receipts::SEEN_DELAY).await;
        let prepared = {
            let Ok(mut guard) = active.lock() else {
                return;
            };
            let Some(room) = guard.as_mut() else {
                return;
            };
            if !room
                .messages
                .iter()
                .any(|message| message.unique_id == unique_id)
            {
                return;
            }
            let reader = receipts::likely_reader(&room.messages);
            room.receipts.push(receipts::Receipt { unique_id, reader });
            let changed = receipts::stamp(&mut room.messages, &room.receipts);
            let patches: Vec<TimelinePatch> = changed
                .into_iter()
                .filter_map(|offset| {
                    let message = room.messages.get(offset)?.clone();
                    Some(TimelinePatch::Set {
                        index: room.prepended.saturating_add(offset),
                        message,
                    })
                })
                .collect();
            (room.timeline_tx.clone(), patches)
        };
        let (timeline_tx, patches) = prepared;
        if !patches.is_empty() {
            send_patch(&timeline_tx, TimelinePatch::Batch(patches)).await;
        }
    });
}

fn spawn_late_sets(
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    messages: Vec<TimelineMessage>,
    rows: Vec<usize>,
    interval: Duration,
) {
    tokio::spawn(async move {
        for index in rows {
            sleep(interval).await;
            let Some(message) = messages.get(index) else {
                return;
            };
            let message = message.clone();
            send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        }
    });
}

fn spawn_poll_activity(active: &SharedActiveRoom, timeline_tx: &mpsc::Sender<TimelineUpdate>) {
    let scenario = polls::scenario();
    if scenario.members_keep_voting {
        let active = Arc::clone(active);
        let timeline_tx = timeline_tx.clone();
        tokio::spawn(async move {
            for round in 0..polls::LIVE_ROUNDS {
                sleep(polls::LIVE_INTERVAL).await;
                let voted = patch_newest_open_poll(&active, &timeline_tx, |message| {
                    polls::member_votes(message, round)
                });
                if !voted.await {
                    return;
                }
            }
        });
    }
    if scenario.newest_poll_ends {
        let active = Arc::clone(active);
        let timeline_tx = timeline_tx.clone();
        tokio::spawn(async move {
            sleep(polls::ENDS_AFTER).await;
            patch_newest_open_poll(&active, &timeline_tx, polls::close).await;
        });
    }
}

async fn patch_newest_open_poll(
    active: &SharedActiveRoom,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
    change: impl FnOnce(&mut TimelineMessage) -> bool + Send,
) -> bool {
    let prepared = {
        let Ok(mut guard) = active.lock() else {
            return false;
        };
        let Some(room) = guard.as_mut() else {
            return false;
        };
        if !room.timeline_tx.same_channel(timeline_tx) {
            return false;
        }
        let Some(offset) = polls::newest_open_poll(&room.messages) else {
            return false;
        };
        let row = room.prepended.saturating_add(offset);
        let Some(message) = room.messages.get_mut(offset) else {
            return false;
        };
        if !change(message) {
            return false;
        }
        (row, message.clone())
    };
    let (index, message) = prepared;
    send_patch(timeline_tx, TimelinePatch::Set { index, message }).await;
    true
}

async fn refuse_poll_action(timeline_tx: &mpsc::Sender<TimelineUpdate>, action: PollAction) {
    tracing::debug!(
        ?action,
        "demo: the room's power levels refuse this poll action"
    );
    drop(
        timeline_tx
            .send(TimelineUpdate::PollSendFailed(action))
            .await,
    );
}

fn spawn_poll_refusal(
    active: SharedActiveRoom,
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    event_id: String,
    undo: Option<polls::Undo>,
    action: PollAction,
) {
    tokio::spawn(async move {
        sleep(polls::REFUSAL_DELAY).await;
        if let Some(undo) = undo {
            let reverted = {
                let Ok(mut guard) = active.lock() else {
                    return;
                };
                let Some(room) = guard.as_mut() else {
                    return;
                };
                if !room.timeline_tx.same_channel(&timeline_tx) {
                    return;
                }
                let Some(offset) = room
                    .messages
                    .iter()
                    .position(|message| message.event_id.as_deref() == Some(event_id.as_str()))
                else {
                    return;
                };
                let row = room.prepended.saturating_add(offset);
                let Some(message) = room.messages.get_mut(offset) else {
                    return;
                };
                if !undo.apply(message) {
                    return;
                }
                (row, message.clone())
            };
            let (index, message) = reverted;
            send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        }
        drop(
            timeline_tx
                .send(TimelineUpdate::PollSendFailed(action))
                .await,
        );
    });
}

fn spawn_edit_refusal(
    active: SharedActiveRoom,
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    edit: MessageEdit,
    previous: Revision,
) {
    tokio::spawn(async move {
        sleep(timeline::EDIT_REFUSAL_DELAY).await;
        let reverted = active.lock().ok().and_then(|mut guard| {
            let room = guard
                .as_mut()
                .filter(|room| room.timeline_tx.same_channel(&timeline_tx))?;
            let offset = room
                .messages
                .iter()
                .position(|message| edit_names(&edit.target, message))?;
            let row = room.prepended.saturating_add(offset);
            let message = room.messages.get_mut(offset)?;
            if message.editable_text() != Some(edit.body.as_str()) {
                return None;
            }
            previous.restore(message);
            Some((row, message.clone()))
        });
        if let Some((index, message)) = reverted {
            send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        }
        let unsaved = MessageEdit {
            original: None,
            ..edit
        };
        drop(timeline_tx.send(TimelineUpdate::EditUnsaved(unsaved)).await);
    });
}

fn spawn_late_append(active: SharedActiveRoom, timeline_tx: mpsc::Sender<TimelineUpdate>) {
    tokio::spawn(async move {
        sleep(timeline::LATE_MESSAGE_DELAY).await;
        let mut message = data::own_message(
            9_000,
            "posted while the timeline was settling",
            &[],
            None,
            SendState::Sent,
        );
        message.is_own = false;
        "@sarah:matrix.org".clone_into(&mut message.sender);
        message.sender_name = MemberName::Unique("Sarah Chen".to_owned());
        let above_pending = active.lock().ok().and_then(|mut guard| {
            let room = guard
                .as_mut()
                .filter(|room| room.timeline_tx.same_channel(&timeline_tx))?;
            land_above_pending_sends(room, &message)
        });
        let arrivals = Arrivals::of([&message]);
        let patch = above_pending.unwrap_or(TimelinePatch::PushBack(message));
        let arrival = TimelineUpdate::Patch {
            patch: Box::new(patch),
            arrivals,
        };
        send_update(&timeline_tx, arrival).await;
    });
}

fn land_above_pending_sends(
    room: &mut ActiveRoom,
    arrival: &TimelineMessage,
) -> Option<TimelinePatch> {
    let pending_sends = room
        .messages
        .iter()
        .rev()
        .take_while(|message| message.local_id.is_some())
        .count();
    if pending_sends == 0 {
        return None;
    }
    let offset = room.messages.len().saturating_sub(pending_sends);
    room.messages.insert(offset, arrival.clone());
    Some(TimelinePatch::Insert {
        index: room.prepended.saturating_add(offset),
        message: arrival.clone(),
    })
}

fn older_history(round: u64, messages: &[TimelineMessage]) -> Vec<TimelineMessage> {
    messages
        .iter()
        .take(timeline::HISTORY_PAGE)
        .enumerate()
        .map(|(index, message)| {
            let id = format!("demo-history-{round}-{index}");
            TimelineMessage {
                unique_id: id.clone(),
                event_id: Some(id),
                local_id: None,
                is_first_unread: false,
                send_state: SendState::default(),
                ..message.clone()
            }
        })
        .collect()
}

fn grown_body(body: &MessageBody, round: usize) -> MessageBody {
    let padding = " and it keeps going".repeat(round % 4);
    match body {
        MessageBody::Text(text) if text.html().is_none() => {
            MessageBody::Text(RichText::plain(format!("{}{padding}", text.plain)))
        }
        other => other.clone(),
    }
}

fn spawn_upload_progress(
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    index: usize,
    settled: TimelineMessage,
    total: u64,
) {
    tokio::spawn(async move {
        for step in 1..=attachments::UPLOAD_STEPS {
            sleep(attachments::UPLOAD_STEP_DELAY).await;
            let mut message = settled.clone();
            message.send_state = SendState::Uploading {
                sent: total * step / attachments::UPLOAD_STEPS,
                total,
            };
            send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        }
        sleep(attachments::UPLOAD_STEP_DELAY).await;
        send_patch(
            &timeline_tx,
            TimelinePatch::Set {
                index,
                message: settled,
            },
        )
        .await;
    });
}

struct RemovedMessage {
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    offset: usize,
    row: usize,
    message: TimelineMessage,
    replies: Vec<(usize, TimelineMessage)>,
}

fn stamp_deleted_reply(message: &mut TimelineMessage, deleted: &HashSet<String>) -> bool {
    let Some(reply) = message.reply.as_mut() else {
        return false;
    };
    if !deleted.contains(&reply.event_id) || reply.kind == MessagePreviewKind::Deleted {
        return false;
    }
    reply.kind = MessagePreviewKind::Deleted;
    reply.body = RichText::default();
    true
}

fn spawn_deletion_refusal(active: SharedActiveRoom, removed: RemovedMessage) {
    tokio::spawn(async move {
        sleep(message_menu::REFUSAL_DELAY).await;
        let restored = {
            let Ok(mut guard) = active.lock() else {
                return;
            };
            let Some(room) = guard.as_mut() else {
                return;
            };
            if !room.timeline_tx.same_channel(&removed.timeline_tx) {
                return;
            }
            let offset = removed.offset.min(room.messages.len());
            room.messages.insert(offset, removed.message.clone());
            room.prepended.saturating_add(offset)
        };
        send_patch(
            &removed.timeline_tx,
            TimelinePatch::Insert {
                index: restored,
                message: removed.message,
            },
        )
        .await;
        drop(removed.timeline_tx.send(TimelineUpdate::DeleteFailed).await);
    });
}

fn spawn_others_keys(room_id: &RoomId) {
    let room_id = room_id.clone();
    let late = calls::scenario().keys_late;
    tokio::spawn(async move {
        if late {
            sleep(calls::KEYS_LATE_BY).await;
        }
        for key in calls::book().keys_from_others(room_id.as_ref()) {
            calls::book().signal(key).await;
        }
    });
}

async fn send_patch(timeline_tx: &mpsc::Sender<TimelineUpdate>, patch: TimelinePatch) {
    send_update(timeline_tx, TimelineUpdate::patch(patch)).await;
}

async fn send_update(timeline_tx: &mpsc::Sender<TimelineUpdate>, update: TimelineUpdate) {
    if let Err(e) = timeline_tx.send(update).await {
        tracing::debug!("demo timeline receiver closed: {e}");
    }
}

async fn demo_oauth_start() -> Result<OAuthLoginData> {
    let page = login::sign_in_page().ok_or_else(|| unavailable("OAuth login"))?;
    login::pause().await;
    let page = Url::parse(page).map_err(|e| AppError::Other(e.to_string()))?;
    Ok(OAuthLoginData {
        auth_url: LauncherSafeUrl::sign_in_page(page)?,
    })
}

fn with_room_facts<T>(room_id: &RoomId, measure: impl FnOnce(&RoomFacts<'_>) -> T) -> T {
    let session = data::session();
    measure(&RoomFacts {
        room_id: room_id.as_ref(),
        sender: &session.user_id,
        device_id: &session.device_id,
        encryption: Encryption::when(data::room_is_encrypted(room_id)),
        event_ids: EventIds::Hashed,
        room_mentions: RoomMentions::when(powers::messages().notify_room),
    })
}

fn unavailable(action: &str) -> AppError {
    AppError::Other(format!("{action} is not available in demo mode"))
}
