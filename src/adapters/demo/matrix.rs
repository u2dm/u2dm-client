use std::fs;
use std::future::pending;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::{data, login, media, stickers, timeline, verification};
use crate::domain::models::{
    AuthMethod, JumpTarget, LoginCredentials, MessageBody, OAuthLoginData, PackId,
    PaginationDirection, PaginationOutcome, ReplyInfo, RoomId, ServerInfo, Session, StickerImage,
    SyncEvent, SyncOutcome, TimelineCommand, TimelineFocus, TimelineMessage, TimelinePatch,
    TimelineUpdate, VerificationCancellation, VerificationEvent,
};
use crate::error::{AppError, Result};
use crate::ports::matrix::{
    AuthPort, AuthenticatedSession, CleanupReport, MediaPort, PendingLogin, ProgressSink,
    RestoreStep, SessionPort, SpaceOrderPort, StickerCatalog, StickerPort, StoreAdoption, SyncPort,
    SyncSink, TimelinePort, VerificationPort,
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
        Ok(data::session())
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

    async fn pending_logins(&self) -> Vec<PendingLogin> {
        Vec::new()
    }

    async fn unwind_login(&self, _txn: &str) -> CleanupReport {
        CleanupReport::default()
    }

    async fn settle_login(&self, _txn: &str) -> CleanupReport {
        CleanupReport::default()
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

    async fn credentials_written(&self) -> Result<()> {
        Ok(())
    }

    async fn rolling_back(&self) -> Result<()> {
        Ok(())
    }

    async fn commit(self: Box<Self>) -> AuthenticatedSession {
        authenticated(self.session)
    }

    async fn roll_back(self: Box<Self>) -> CleanupReport {
        CleanupReport::default()
    }
}

fn authenticated(session: Session) -> AuthenticatedSession {
    let authed = Arc::new(DemoAuthed::default());
    let sync = Arc::clone(&authed);
    let timeline = Arc::clone(&authed);
    let media = Arc::clone(&authed);
    let verification = Arc::clone(&authed);
    let space_order = Arc::clone(&authed);
    let stickers = Arc::clone(&authed);
    AuthenticatedSession {
        session,
        sync,
        timeline,
        media,
        verification,
        space_order,
        stickers,
        lifecycle: authed,
    }
}

struct ActiveRoom {
    room_id: RoomId,
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    messages: Vec<TimelineMessage>,
    prepended: usize,
}

#[derive(Default)]
struct DemoAuthed {
    active: Mutex<Option<ActiveRoom>>,
    sent: AtomicU64,
    verification_tx: Mutex<Option<mpsc::UnboundedSender<VerificationEvent>>>,
}

impl DemoAuthed {
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
            let message = data::own_message(self.sent.fetch_add(1, Ordering::Relaxed), body, reply);
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
        on_sync(SyncEvent::Connected);
        on_sync(SyncEvent::Rooms(data::rooms().into()));
        on_sync(SyncEvent::Spaces(data::spaces().into()));
        if !timeline::scenario().room_list_keeps_updating {
            cancel.cancelled().await;
            return SyncOutcome::Cancelled;
        }
        loop {
            tokio::select! {
                () = cancel.cancelled() => return SyncOutcome::Cancelled,
                () = sleep(timeline::ROOM_LIST_INTERVAL) => {
                    on_sync(SyncEvent::Rooms(data::rooms().into()));
                }
            }
        }
    }
}

#[async_trait]
impl SpaceOrderPort for DemoAuthed {
    async fn set_space_order(&self, space_id: &RoomId, order: &str) -> Result<()> {
        tracing::debug!(%space_id, order, "demo: ignoring space order write");
        Ok(())
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
        let messages = match focus.target() {
            Some(target) => focused_window(&all, target)?,
            None if scenario.window_is_short => newest(&all, timeline::SHORT_WINDOW),
            None => all,
        };

        if scenario.resolving_unread && focus.is_live() {
            drop(timeline_tx.send(TimelineUpdate::ResolvingUnread).await);
        }
        if scenario.reset_is_slow {
            sleep(timeline::SLOW_RESET_DELAY).await;
        }
        send_patch(&timeline_tx, opening_patch(scenario, messages.clone())).await;

        if let Ok(mut active) = self.active.lock() {
            *active = Some(ActiveRoom {
                room_id: room_id.clone(),
                timeline_tx: timeline_tx.clone(),
                messages: messages.clone(),
                prepended: 0,
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

        if focus.is_live() && scenario.reset_repeats {
            spawn_late_reset(scenario, timeline_tx.clone(), messages.clone());
        }
        if scenario.rows_keep_resizing {
            spawn_row_churn(timeline_tx.clone(), messages.clone());
        }
        if scenario.message_arrives_late {
            spawn_late_append(timeline_tx.clone());
        }

        let mut history = 0_u64;
        while let Some(command) = cmd_rx.recv().await {
            let direction = match command {
                TimelineCommand::PaginateBackwards => PaginationDirection::Backwards,
                TimelineCommand::PaginateForwards => PaginationDirection::Forwards,
                TimelineCommand::MarkRead => continue,
                TimelineCommand::JumpTo(event_id) => {
                    let target = self.loaded_row_of(&event_id);
                    drop(
                        timeline_tx
                            .send(TimelineUpdate::JumpOutcome { event_id, target })
                            .await,
                    );
                    continue;
                }
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
                for message in page {
                    send_patch(&timeline_tx, TimelinePatch::PushFront(message)).await;
                }
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
        self.append_own_message(room_id, body, None).await;
        Ok(())
    }

    async fn send_reply(&self, room_id: &RoomId, body: &str, in_reply_to: &str) -> Result<()> {
        self.append_own_message(room_id, body, Some(in_reply_to))
            .await;
        Ok(())
    }
}

#[async_trait]
impl MediaPort for DemoAuthed {
    async fn download_media(&self, event_id: &str, _thumbnail: bool) -> Result<Vec<u8>> {
        let path: PathBuf = media::DemoMediaCache
            .thumbnail_path(event_id)
            .ok_or_else(|| AppError::Other(format!("no demo asset for event {event_id}")))?;
        Ok(fs::read(path)?)
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
            room_encrypted: demo.room_is_encrypted,
        })
    }

    async fn prefetch(&self, mxcs: &[String]) -> usize {
        stickers::pause_prefetch().await;
        mxcs.iter()
            .filter(|mxc| media::DemoMediaCache.sticker_path(mxc).is_some())
            .count()
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

fn spawn_late_append(timeline_tx: mpsc::Sender<TimelineUpdate>) {
    tokio::spawn(async move {
        sleep(timeline::LATE_MESSAGE_DELAY).await;
        let mut message = data::own_message(9_000, "posted while the timeline was settling", None);
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
                is_first_unread: false,
                ..message.clone()
            }
        })
        .collect()
}

fn grown_body(body: &MessageBody, round: usize) -> MessageBody {
    let padding = " and it keeps going".repeat(round % 4);
    match body {
        MessageBody::Text(text) => MessageBody::Text(format!("{text}{padding}")),
        other => other.clone(),
    }
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
