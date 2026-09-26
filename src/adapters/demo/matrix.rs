use std::fs;
use std::future::pending;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::spawn_blocking;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::{
    attachments, audio, data, login, media, pinned, polls, reactions, receipts, space_index,
    stickers, timeline, verification, videos,
};
use crate::adapters::video;
use crate::domain::auth::{AuthMethod, LoginCredentials, OAuthLoginData, ServerInfo, Session};
use crate::domain::media::{MediaRendition, OutgoingAttachment, WaveformNeed};
use crate::domain::message::{
    MessageBody, PinnedMessage, ReplyInfo, RichText, SendState, TimelineMessage,
};
use crate::domain::poll::{PollAction, PollDraft};
use crate::domain::room::RoomId;
use crate::domain::space_index::HierarchyPage;
use crate::domain::sticker::{PackId, StickerImage};
use crate::domain::sync::{SyncEvent, SyncOutcome};
use crate::domain::timeline::{
    AudioLookup, AudioTrack, JumpTarget, PaginationDirection, PaginationOutcome, TimelineCommand,
    TimelineFocus, TimelinePatch, TimelineUpdate, locate_audio,
};
use crate::domain::verification::{VerificationCancellation, VerificationEvent};
use crate::error::{AppError, Result};
use crate::ports::matrix::{
    AuthPort, AuthenticatedSession, CleanupReport, InterruptedLogin, LocalDataOwnership, MediaPort,
    PinnedPort, ProgressSink, RestoreStep, SessionPort, SpaceIndexPort, SpaceOrderPort,
    StickerCatalog, StickerPort, StoreAdoption, SyncPort, SyncSink, TimelinePort, VerificationPort,
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
        if !login::oauth_succeeds() {
            return Err(unavailable("OAuth login"));
        }
        login::pause().await;
        Ok(OAuthLoginData {
            auth_url: "https://example.invalid/demo-oauth".to_owned(),
        })
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
        if !login::oauth_succeeds() {
            return Err(unavailable("OAuth login"));
        }
        login::pause().await;
        Ok(OAuthLoginData {
            auth_url: "https://example.invalid/demo-oauth".to_owned(),
        })
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
        stickers,
        lifecycle: authed,
    }
}

struct ActiveRoom {
    room_id: RoomId,
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    messages: Vec<TimelineMessage>,
    prepended: usize,
    receipts: Vec<receipts::Receipt>,
}

type SharedActiveRoom = Arc<Mutex<Option<ActiveRoom>>>;

#[derive(Default)]
struct DemoAuthed {
    active: SharedActiveRoom,
    sent: AtomicU64,
    verification_tx: Mutex<Option<mpsc::UnboundedSender<VerificationEvent>>>,
    sync_sink: Mutex<Option<SyncSink>>,
    joined: Mutex<Vec<RoomId>>,
}

impl DemoAuthed {
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
                message.send_state = SendState::Sent;
                message.event_id = Some(message.unique_id.clone());
                message.local_id = None;
                TimelinePatch::Set {
                    index,
                    message: message.clone(),
                }
            } else {
                active.messages.remove(index);
                TimelinePatch::Remove { index }
            };
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

            let reply = in_reply_to.and_then(|event_id| reply_info(&active.messages, event_id));
            let message = data::own_message(
                self.sent.fetch_add(1, Ordering::Relaxed),
                body,
                reply,
                outgoing_send_state(),
            );
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

            let reply = in_reply_to.and_then(|event_id| reply_info(&active.messages, event_id));
            let message =
                data::own_sticker(self.sent.fetch_add(1, Ordering::Relaxed), image, reply);
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
                .and_then(|event_id| reply_info(&active.messages, event_id));
            let settled =
                data::own_attachment(self.sent.fetch_add(1, Ordering::Relaxed), attachment, reply);
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
        }
        None
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
                Some(previous),
                PollAction::Vote,
            );
        }
    }

    async fn end_poll(&self, event_id: &str, timeline_tx: &mpsc::Sender<TimelineUpdate>) {
        if !polls::permissions().end {
            refuse_poll_action(timeline_tx, PollAction::End).await;
            return;
        }
        let Some((timeline_tx, index, message)) = self.patch_poll(event_id, polls::end_own) else {
            tracing::debug!(event_id, "demo: only an own open poll can be ended");
            return;
        };
        send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        if polls::scenario().ends_fail {
            spawn_poll_refusal(
                Arc::clone(&self.active),
                timeline_tx,
                event_id.to_owned(),
                None,
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

    fn locate_audio(&self, lookup: &AudioLookup) -> Option<AudioTrack> {
        let guard = self.active.lock().ok()?;
        let active = guard.as_ref()?;
        locate_audio(lookup, active.messages.iter().cloned())
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
}

#[async_trait]
impl SyncPort for DemoAuthed {
    async fn start_sync(&self, on_sync: SyncSink, cancel: CancellationToken) -> SyncOutcome {
        if let Ok(mut sink) = self.sync_sink.lock() {
            *sink = Some(Arc::clone(&on_sync));
        }
        let joined = self.joined_rooms();
        on_sync(SyncEvent::Connected);
        on_sync(SyncEvent::Rooms(data::rooms_with(&joined).into()));
        on_sync(SyncEvent::Spaces(data::spaces_with(&joined).into()));
        if !timeline::scenario().room_list_keeps_updating {
            cancel.cancelled().await;
            return SyncOutcome::Cancelled;
        }
        loop {
            tokio::select! {
                () = cancel.cancelled() => return SyncOutcome::Cancelled,
                () = sleep(timeline::ROOM_LIST_INTERVAL) => {
                    on_sync(SyncEvent::Rooms(data::rooms_with(&self.joined_rooms()).into()));
                }
            }
        }
    }

    fn set_selected_room(&self, room_id: Option<&RoomId>) {
        tracing::debug!(?room_id, "demo: the selected room needs no subscription");
    }
}

impl DemoAuthed {
    fn joined_rooms(&self) -> Vec<RoomId> {
        self.joined
            .lock()
            .map(|joined| joined.clone())
            .unwrap_or_default()
    }

    fn remember_join(&self, room_id: &RoomId) -> Vec<RoomId> {
        let Ok(mut joined) = self.joined.lock() else {
            return Vec::new();
        };
        if !joined.contains(room_id) {
            joined.push(room_id.clone());
        }
        joined.clone()
    }

    fn spawn_poll_revocation(&self) {
        if !polls::scenario().permissions_get_revoked {
            return;
        }
        let Some(sink) = self.sync_sink.lock().ok().and_then(|sink| sink.clone()) else {
            return;
        };
        let joined = self.joined_rooms();
        tokio::spawn(async move {
            sleep(polls::REVOKED_AFTER).await;
            if polls::revoke() {
                sink(SyncEvent::Rooms(data::rooms_with(&joined).into()));
            }
        });
    }

    fn echo_join(&self, joined: Vec<RoomId>) {
        let Some(sink) = self.sync_sink.lock().ok().and_then(|sink| sink.clone()) else {
            return;
        };
        tokio::spawn(async move {
            sleep(space_index::JOIN_ECHO_LAG).await;
            sink(SyncEvent::Rooms(data::rooms_with(&joined).into()));
            sink(SyncEvent::Spaces(data::spaces_with(&joined).into()));
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
        let joined = self.remember_join(room_id);
        self.echo_join(joined);
        Ok(())
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        space_index::pause_avatars().await;
        media::fetch_unjoined_avatars(mxcs)
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

#[async_trait]
impl TimelinePort for DemoAuthed {
    async fn subscribe_timeline(
        &self,
        room_id: &RoomId,
        focus: TimelineFocus,
        timeline_tx: mpsc::Sender<TimelineUpdate>,
        mut cmd_rx: mpsc::UnboundedReceiver<TimelineCommand>,
    ) -> Result<()> {
        let scenario = timeline::scenario();
        let all = data::messages(room_id);
        let seeded_receipts = receipts::seed_receipts(&all);
        let messages = opening_window(all, &focus, scenario)?;

        if scenario.resolving_unread && focus.opens_at_read_position() {
            drop(timeline_tx.send(TimelineUpdate::ResolvingUnread).await);
        }
        if scenario.unread_boundary_is_unresolved && focus.opens_at_read_position() {
            drop(timeline_tx.send(TimelineUpdate::UnreadUnresolved).await);
        }
        if scenario.reset_is_slow {
            sleep(timeline::SLOW_RESET_DELAY).await;
        }
        let reset_messages = reset_messages(&messages);
        send_patch(
            &timeline_tx,
            opening_patch(scenario, reset_messages.clone()),
        )
        .await;

        if let Ok(mut active) = self.active.lock() {
            *active = Some(ActiveRoom {
                room_id: room_id.clone(),
                timeline_tx: timeline_tx.clone(),
                messages: messages.clone(),
                prepended: 0,
                receipts: seeded_receipts,
            });
        }

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

        spawn_scenario_tasks(scenario, &focus, &timeline_tx, &messages, reset_messages);
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

    async fn send_attachment(
        &self,
        room_id: &RoomId,
        attachment: &OutgoingAttachment,
    ) -> Result<()> {
        if attachments::scenario().refuses_size {
            return Err(AppError::AttachmentTooLarge {
                limit: attachments::upload_limit(),
            });
        }
        if attachments::scenario().send_fails {
            attachments::pause_upload().await;
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
            return Ok(());
        };
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
        if uploads_slowly && let Some(timeline_tx) = self.timeline_sender(room_id) {
            spawn_upload_progress(timeline_tx, index, settled, total);
        }
        Ok(())
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
        let pins = data::pinned_messages(room_id);
        if pinned_tx.send(pins.clone()).await.is_err() {
            return Ok(());
        }
        if pinned::scenario().repins
            && let Some(newest) = data::latest_pinnable(room_id)
        {
            repin(&pinned_tx, &pins, newest).await;
        }
        pinned_tx.closed().await;
        Ok(())
    }
}

async fn repin(
    pinned_tx: &mpsc::Sender<Vec<PinnedMessage>>,
    pins: &[PinnedMessage],
    newest: PinnedMessage,
) {
    let unpinned: Vec<PinnedMessage> = pins
        .iter()
        .filter(|pin| pin.event_id != newest.event_id)
        .cloned()
        .collect();
    let mut repinned = unpinned.clone();
    repinned.push(newest);
    let mut pinned_now = false;
    loop {
        tokio::select! {
            () = sleep(pinned::REPIN_INTERVAL) => {}
            () = pinned_tx.closed() => return,
        }
        pinned_now = !pinned_now;
        let next = if pinned_now {
            repinned.clone()
        } else {
            unpinned.clone()
        };
        if pinned_tx.send(next).await.is_err() {
            return;
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
        let Some(demo) = verification::requested() else {
            return Err(unavailable("Verification"));
        };
        verification::pause().await;
        if demo.fails == verification::FailingStep::Reject {
            return Err(unavailable("Declining a verification"));
        }
        self.emit_verification(VerificationEvent::Cancelled(
            VerificationCancellation::Declined,
        ));
        Ok(())
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
    all: Vec<TimelineMessage>,
    focus: &TimelineFocus,
    scenario: timeline::Scenario,
) -> Result<Vec<TimelineMessage>> {
    let mut messages = match focus.target() {
        Some(target) => focused_window(&all, target)?,
        None if scenario.window_is_short => newest(&all, timeline::SHORT_WINDOW),
        None => all,
    };
    if matches!(focus, TimelineFocus::Latest) {
        clear_unread_divider(&mut messages);
    }
    Ok(messages)
}

fn clear_unread_divider(messages: &mut [TimelineMessage]) {
    for message in messages {
        message.is_first_unread = false;
    }
}

fn newest(messages: &[TimelineMessage], count: usize) -> Vec<TimelineMessage> {
    let start = messages.len().saturating_sub(count);
    messages.get(start..).unwrap_or(messages).to_vec()
}

fn focused_window(messages: &[TimelineMessage], target: &str) -> Result<Vec<TimelineMessage>> {
    let index = messages
        .iter()
        .position(|message| message.event_id.as_deref() == Some(target))
        .ok_or_else(|| AppError::Other(format!("no demo event {target}")))?;
    let start = index.saturating_sub(timeline::FOCUS_CONTEXT);
    let end = messages
        .len()
        .min(index.saturating_add(timeline::FOCUS_CONTEXT));
    Ok(messages.get(start..end).unwrap_or(messages).to_vec())
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
        spawn_late_append(timeline_tx.clone());
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
    restore: Option<Vec<String>>,
    action: PollAction,
) {
    tokio::spawn(async move {
        sleep(polls::REFUSAL_DELAY).await;
        if let Some(selection) = restore {
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
                if !polls::restore(message, &selection) {
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

fn spawn_late_append(timeline_tx: mpsc::Sender<TimelineUpdate>) {
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
        send_patch(&timeline_tx, TimelinePatch::PushBack(message)).await;
    });
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
        MessageBody::Text(text) if text.html.is_none() => {
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

async fn send_patch(timeline_tx: &mpsc::Sender<TimelineUpdate>, patch: TimelinePatch) {
    if let Err(e) = timeline_tx
        .send(TimelineUpdate::Patch(Box::new(patch)))
        .await
    {
        tracing::debug!("demo timeline receiver closed: {e}");
    }
}

fn unavailable(action: &str) -> AppError {
    AppError::Other(format!("{action} is not available in demo mode"))
}
