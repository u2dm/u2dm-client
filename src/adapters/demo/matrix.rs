use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::sync::mpsc;

use super::{data, media};
use crate::domain::models::{
    AuthMethod, LoginCredentials, OAuthLoginData, PaginationDirection, PaginationOutcome,
    ReplyInfo, RoomId, ServerInfo, Session, SyncEvent, TimelineCommand, TimelineMessage,
    TimelinePatch, TimelineUpdate, VerificationEvent,
};
use crate::error::{AppError, Result};
use crate::ports::matrix::MatrixPort;
use crate::ports::media::MediaCache;

struct ActiveRoom {
    room_id: RoomId,
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    messages: Vec<TimelineMessage>,
}

#[derive(Default)]
pub struct DemoMatrix {
    active: Mutex<Option<ActiveRoom>>,
    sent: AtomicU64,
}

impl DemoMatrix {
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
impl MatrixPort for DemoMatrix {
    async fn discover_auth(&self, homeserver: &str, _passphrase: &str) -> Result<ServerInfo> {
        Ok(ServerInfo {
            auth_methods: vec![AuthMethod::Password],
            homeserver_url: format!("https://{homeserver}"),
        })
    }

    async fn login_password(&self, _creds: LoginCredentials) -> Result<Session> {
        Ok(data::session())
    }

    async fn login_oauth_start(&self) -> Result<OAuthLoginData> {
        Err(unavailable("OAuth login"))
    }

    async fn login_oauth_finish(&self) -> Result<Session> {
        Err(unavailable("OAuth login"))
    }

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

    async fn start_sync(&self, on_sync: Box<dyn Fn(SyncEvent) + Send + Sync>) -> Result<()> {
        on_sync(SyncEvent::Connected);
        on_sync(SyncEvent::Rooms(data::rooms().into()));
        on_sync(SyncEvent::Spaces(data::spaces().into()));
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

    async fn download_media(&self, event_id: &str, _thumbnail: bool) -> Result<Vec<u8>> {
        let path: PathBuf = media::DemoMediaCache
            .thumbnail_path(event_id)
            .ok_or_else(|| AppError::Other(format!("no demo asset for event {event_id}")))?;
        Ok(fs::read(path)?)
    }

    async fn fetch_user_avatar(&self) -> Result<Option<PathBuf>> {
        Ok(media::user_avatar_path())
    }

    async fn restore_session(
        &self,
        _session: &Session,
        _passphrase: &str,
        _on_progress: Box<dyn Fn(String) + Send + Sync>,
    ) -> Result<()> {
        Ok(())
    }

    async fn logout(&self) -> Result<()> {
        Ok(())
    }

    async fn clear_store(&self) -> Result<()> {
        Ok(())
    }

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

    async fn subscribe_session_changes(
        &self,
        _session_tx: mpsc::UnboundedSender<Session>,
    ) -> Result<()> {
        Ok(())
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
