use std::collections::{HashMap, HashSet};
use std::fs;
use std::future::pending;
use std::mem;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{mpsc, watch};
use tokio::task::spawn_blocking;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{
    attachments, audio, data, login, media, message_menu, pinned, polls, reactions, receipts,
    room_info, source, space_index, stickers, timeline, user_info, verification, videos,
};
use crate::adapters::video;
use crate::domain::auth::{AuthMethod, LoginCredentials, OAuthLoginData, ServerInfo, Session};
use crate::domain::link::LauncherSafeUrl;
use crate::domain::media::{MediaRendition, OutgoingAttachment, WaveformNeed};
use crate::domain::message::{
    EditTarget, MessageBody, MessageEdit, MessagePreviewKind, PinChange, PinnedMessage, ReplyInfo,
    RichText, SendState, TimelineMessage,
};
use crate::domain::poll::{PollAction, PollDraft};
use crate::domain::room::{NotifyMode, Room, RoomId, Space};
use crate::domain::room_info::{Reader, RoomAbout, RosterMember};
use crate::domain::space_index::HierarchyPage;
use crate::domain::sticker::{PackId, StickerImage};
use crate::domain::sync::{SyncEvent, SyncOutcome};
use crate::domain::timeline::{
    AudioLookup, AudioTrack, EventSource, JumpTarget, Landing, MessageReaders, PaginationDirection,
    PaginationOutcome, TimelineCommand, TimelineFocus, TimelinePatch, TimelineUpdate, locate_audio,
};
use crate::domain::user_info::{
    GlobalProfile, IgnoreChange, Moderation, RoomMembership, UserId, UserProfile,
};
use crate::domain::verification::{VerificationCancellation, VerificationEvent};
use crate::error::{AppError, Result};
use crate::ports::matrix::{
    AttachmentHandoff, AuthPort, AuthenticatedSession, CleanupReport, InterruptedLogin,
    LocalDataOwnership, MediaPort, PinnedPort, ProgressSink, RestoreStep, RoomInfoPort,
    SessionPort, SpaceIndexPort, SpaceOrderPort, StickerCatalog, StickerPort, StoreAdoption,
    SyncPort, SyncSink, TimelinePort, UserInfoPort, VerificationPort,
};
use crate::ports::media::MediaCache;

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
    ) -> Result<AuthenticatedSession> {
        for step in [RestoreStep::Connecting, RestoreStep::RestoringAuth] {
            on_progress(step);
            login::pause().await;
        }
        Ok(authenticated(session.clone()))
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
    let space_order = Arc::clone(&authed);
    let space_index = Arc::clone(&authed);
    let room_info = Arc::clone(&authed);
    let user_info = Arc::clone(&authed);
    let stickers = Arc::clone(&authed);
    AuthenticatedSession {
        session,
        sync,
        timeline,
        pinned,
        media,
        verification,
        space_order,
        space_index,
        room_info,
        user_info,
        stickers,
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
        data::spaces_with(&self.joined)
    }
}

fn current_lists(lists: &Mutex<RoomLists>) -> RoomLists {
    lists.lock().map(|lists| lists.clone()).unwrap_or_default()
}

struct Revision {
    body: MessageBody,
    mentions_room: bool,
    edited: bool,
}

impl Revision {
    fn restore(self, message: &mut TimelineMessage) {
        message.body = self.body;
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
        _ => None,
    }
}

fn revised_caption(text: &str, sent: RichText) -> Option<RichText> {
    (!text.is_empty()).then_some(sent)
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
    verification_tx: Mutex<Option<mpsc::UnboundedSender<VerificationEvent>>>,
    sas_started: AtomicBool,
    sync_sink: Mutex<Option<SyncSink>>,
    lists: Arc<Mutex<RoomLists>>,
    pin_boards: Mutex<HashMap<RoomId, watch::Sender<Vec<String>>>>,
    deleted: Arc<Mutex<HashSet<String>>>,
    ignored: Mutex<HashSet<String>>,
    memberships: Mutex<HashMap<(RoomId, String), RoomMembership>>,
}

impl DemoAuthed {
    fn room_history(&self, room_id: &RoomId) -> Vec<TimelineMessage> {
        let mut history = data::messages(room_id);
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
        let sent = data::sent_text(&edit.body);
        let revised = revised_body(&message.body, &edit.body, sent.text)?;
        let previous = Revision {
            body: mem::replace(&mut message.body, revised),
            mentions_room: message.mentions_room,
            edited: message.edited,
        };
        message.mentions_room |= sent.mentions_room;
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
        reset_slot.send(TimelineUpdate::Patch(Box::new(reset)));
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

    async fn append_own_message(&self, room_id: &RoomId, body: &str, in_reply_to: Option<&str>) {
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
                reply,
                outgoing_send_state(),
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
                outgoing_send_state(),
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
        image: &StickerImage,
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
            TimelineCommand::ToggleReaction { event_id, key } => {
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
        if !polls::permissions().vote {
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
        if !polls::permissions().end {
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

    async fn edit_poll(
        &self,
        event_id: &str,
        draft: &PollDraft,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) {
        if !polls::permissions().start {
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
}

#[async_trait]
impl SyncPort for DemoAuthed {
    async fn start_sync(&self, on_sync: SyncSink, cancel: CancellationToken) -> SyncOutcome {
        if let Ok(mut sink) = self.sync_sink.lock() {
            *sink = Some(Arc::clone(&on_sync));
        }
        let lists = current_lists(&self.lists);
        on_sync(SyncEvent::Connected);
        on_sync(SyncEvent::Rooms(lists.rooms().into()));
        on_sync(SyncEvent::Spaces(lists.spaces().into()));
        if !timeline::scenario().room_list_keeps_updating {
            cancel.cancelled().await;
            return SyncOutcome::Cancelled;
        }
        loop {
            tokio::select! {
                () = cancel.cancelled() => return SyncOutcome::Cancelled,
                () = sleep(timeline::ROOM_LIST_INTERVAL) => {
                    on_sync(SyncEvent::Rooms(current_lists(&self.lists).rooms().into()));
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

    fn echo_join(&self) {
        let Some(sink) = self.sync_sink() else {
            return;
        };
        let lists = Arc::clone(&self.lists);
        tokio::spawn(async move {
            sleep(space_index::JOIN_ECHO_LAG).await;
            let lists = current_lists(&lists);
            sink(SyncEvent::Rooms(lists.rooms().into()));
            sink(SyncEvent::Spaces(lists.spaces().into()));
        });
    }

    fn person(&self, room_id: &RoomId, user_id: &UserId) -> UserProfile {
        let mut profile = data::profile(room_id, user_id, user_info::avatar);
        profile.direct_room = self.chat_with(user_id);
        profile.ignored = self.is_ignored(user_id);
        user_info::shape(&mut profile);
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
impl RoomInfoPort for DemoAuthed {
    async fn about(&self, room_id: &RoomId) -> Result<RoomAbout> {
        data::room_about(room_id).ok_or_else(|| unavailable("this room's details"))
    }

    async fn roster(&self, room_id: &RoomId) -> Result<Vec<RosterMember>> {
        room_info::pause().await;
        if room_info::roster_fails_now(room_id) {
            return Err(unavailable("this room's member list"));
        }
        Ok(data::roster(room_id, room_info::member_avatar))
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

    async fn leave(&self, room_id: &RoomId) -> Result<()> {
        room_info::pause().await;
        if room_info::scenario().leave_fails {
            return Err(unavailable("leaving rooms"));
        }
        tracing::debug!(%room_id, "demo: leaving a room");
        self.change_lists(|lists| {
            lists.joined.retain(|joined| joined != room_id);
            lists.overrides.left.insert(room_id.to_string());
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

    async fn moderate(&self, room_id: &RoomId, user_id: &UserId, action: Moderation) -> Result<()> {
        user_info::pause().await;
        if user_info::moderation_fails(action) {
            return Err(unavailable("moderating this room"));
        }
        if !self.person(room_id, user_id).offers(action) {
            return Err(unavailable("that moderation"));
        }
        tracing::debug!(%room_id, %user_id, ?action, "demo: moderating");
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

    async fn send_text(&self, room_id: &RoomId, body: &str) -> Result<()> {
        if timeline::scenario().sends_are_refused {
            return Err(unavailable("sending messages"));
        }
        self.append_own_message(room_id, body, None).await;
        Ok(())
    }

    async fn send_reply(&self, room_id: &RoomId, body: &str, in_reply_to: &str) -> Result<()> {
        if timeline::scenario().sends_are_refused {
            return Err(unavailable("sending replies"));
        }
        self.append_own_message(room_id, body, Some(in_reply_to))
            .await;
        Ok(())
    }

    async fn edit_message(&self, room_id: &RoomId, edit: &MessageEdit) -> Result<()> {
        if timeline::scenario().edits_are_refused {
            return Err(unavailable("editing messages"));
        }
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
        if timeline::scenario().sends_are_refused {
            return Err(unavailable("sending polls"));
        }
        self.append_own_poll(room_id, draft).await;
        Ok(())
    }

    async fn resend(&self, room_id: &RoomId, local_id: &str) -> Result<()> {
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
        let permissions = message_menu::permissions();
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
        if attachments::scenario().refuses_size {
            return Err(AppError::AttachmentTooLarge {
                limit: attachments::upload_limit(),
            });
        }
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
        if !message_menu::permissions().pin {
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
impl StickerPort for DemoAuthed {
    async fn catalog(&self, room_id: &RoomId) -> Result<StickerCatalog> {
        let demo = stickers::scenario();
        stickers::pause_catalog().await;
        if demo.catalog_fails {
            return Err(unavailable("loading sticker packs"));
        }
        let packs = if demo.catalog_is_empty {
            Vec::new()
        } else {
            data::sticker_packs(room_id)
        };
        Ok(StickerCatalog {
            packs,
            room_encrypted: demo.room_is_encrypted || data::room_is_encrypted(room_id),
        })
    }

    async fn prefetch(&self, mxcs: &[String]) -> usize {
        stickers::pause_prefetch().await;
        media::prefetch_stickers(mxcs)
    }

    async fn send_sticker(
        &self,
        room_id: &RoomId,
        pack: &PackId,
        shortcode: &str,
        in_reply_to: Option<&str>,
    ) -> Result<()> {
        if stickers::scenario().send_fails {
            return Err(unavailable("sending stickers"));
        }
        let image = data::sticker_image(pack, shortcode)
            .ok_or_else(|| AppError::Other(format!("no demo sticker {pack}/{shortcode}")))?;
        self.append_own_sticker(room_id, &image, in_reply_to).await;
        Ok(())
    }
}

#[async_trait]
impl VerificationPort for DemoAuthed {
    async fn listen_for_verification(
        &self,
        tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Result<()> {
        let Some(demo) = verification::requested() else {
            return Ok(());
        };
        if let Ok(mut guard) = self.verification_tx.lock() {
            *guard = Some(tx);
        }

        verification::wait_for_request().await;
        self.sas_started.store(false, Ordering::Relaxed);
        self.emit_verification(verification::request(demo));

        if demo.times_out {
            verification::wait_for_request().await;
            self.emit_verification(VerificationEvent::Cancelled(
                VerificationCancellation::TimedOut,
            ));
        }

        pending::<()>().await;
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

    async fn logout(&self) -> Result<()> {
        Ok(())
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
            sender: data::sender_label(message),
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
        .ok_or_else(|| AppError::Other(format!("no demo event {target}")))?;
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
            None,
            SendState::Sent,
        );
        message.is_own = false;
        "@sarah:matrix.org".clone_into(&mut message.sender);
        message.sender_display_name = Some("Sarah Chen".to_owned());
        let above_pending = active.lock().ok().and_then(|mut guard| {
            let room = guard
                .as_mut()
                .filter(|room| room.timeline_tx.same_channel(&timeline_tx))?;
            land_above_pending_sends(room, &message)
        });
        let patch = above_pending.unwrap_or(TimelinePatch::PushBack(message));
        send_patch(&timeline_tx, patch).await;
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
        landing: Landing::AfterRemoteEvents,
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
                landing: Landing::AmongRemoteEvents,
            },
        )
        .await;
        drop(removed.timeline_tx.send(TimelineUpdate::DeleteFailed).await);
    });
}

async fn send_patch(timeline_tx: &mpsc::Sender<TimelineUpdate>, patch: TimelinePatch) {
    if let Err(e) = timeline_tx
        .send(TimelineUpdate::Patch(Box::new(patch)))
        .await
    {
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

fn unavailable(action: &str) -> AppError {
    AppError::Other(format!("{action} is not available in demo mode"))
}
