use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{data, login, media};
use crate::domain::models::{
    AuthMethod, LoginCredentials, OAuthLoginData, PaginationDirection, PaginationOutcome,
    ReplyInfo, RoomId, ServerInfo, Session, SyncEvent, SyncOutcome, TimelineCommand,
    TimelineMessage, TimelinePatch, TimelineUpdate, VerificationEvent,
};
use crate::error::{AppError, Result};
use crate::ports::matrix::{
    AuthPort, AuthenticatedSession, CleanupReport, MediaPort, ProgressSink, RestoreStep,
    SessionPort, SpaceOrderPort, StoreAdoption, SyncPort, SyncSink, TimelinePort, VerificationPort,
};
use crate::ports::media::MediaCache;

pub struct DemoMatrix;

#[async_trait]
impl AuthPort for DemoMatrix {
    async fn discover_auth(&self, homeserver: &str, _passphrase: &str) -> Result<ServerInfo> {
        login::pause().await;
        let auth_methods = login::requested()
            .map_or_else(|| vec![AuthMethod::Password], |demo| demo.methods.clone());
        Ok(ServerInfo {
            auth_methods,
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
        Ok(Box::new(DemoAdoption(session.clone())))
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
}

struct DemoAdoption(Session);

#[async_trait]
impl StoreAdoption for DemoAdoption {
    async fn commit(self: Box<Self>) -> AuthenticatedSession {
        authenticated(self.0)
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
    AuthenticatedSession {
        session,
        sync,
        timeline,
        media,
        verification,
        space_order,
        lifecycle: authed,
    }
}

struct ActiveRoom {
    room_id: RoomId,
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    messages: Vec<TimelineMessage>,
}

#[derive(Default)]
struct DemoAuthed {
    active: Mutex<Option<ActiveRoom>>,
    sent: AtomicU64,
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
}

#[async_trait]
impl SyncPort for DemoAuthed {
    async fn start_sync(&self, on_sync: SyncSink, cancel: CancellationToken) -> SyncOutcome {
        on_sync(SyncEvent::Connected);
        on_sync(SyncEvent::Rooms(data::rooms().into()));
        on_sync(SyncEvent::Spaces(data::spaces().into()));
        cancel.cancelled().await;
        SyncOutcome::Cancelled
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
        timeline_tx: mpsc::Sender<TimelineUpdate>,
        mut cmd_rx: mpsc::UnboundedReceiver<TimelineCommand>,
    ) -> Result<()> {
        let messages = data::messages(room_id);
        send_patch(&timeline_tx, TimelinePatch::Reset(messages.clone())).await;

        if let Ok(mut active) = self.active.lock() {
            *active = Some(ActiveRoom {
                room_id: room_id.clone(),
                timeline_tx: timeline_tx.clone(),
                messages,
            });
        }

        while let Some(command) = cmd_rx.recv().await {
            let direction = match command {
                TimelineCommand::PaginateBackwards => PaginationDirection::Backwards,
                TimelineCommand::PaginateForwards => PaginationDirection::Forwards,
            };
            let update = TimelineUpdate::Pagination {
                direction,
                outcome: PaginationOutcome::Completed { hit_end: true },
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
impl VerificationPort for DemoAuthed {
    async fn listen_for_verification(
        &self,
        _tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Result<()> {
        Ok(())
    }

    async fn accept_verification(&self) -> Result<()> {
        Err(unavailable("Verification"))
    }

    async fn confirm_verification(&self) -> Result<()> {
        Err(unavailable("Verification"))
    }

    async fn reject_verification(&self) -> Result<()> {
        Err(unavailable("Verification"))
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
        .find(|message| message.event_id.as_ref().is_some_and(|id| id.0 == event_id))
        .map(|message| ReplyInfo {
            sender: data::sender_label(message),
            kind: message.body.preview_kind(),
            body: data::body_preview(&message.body),
        })
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
