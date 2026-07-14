use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::sync::mpsc;

use super::{data, media};
use crate::domain::models::{
    AuthMethod, LoginCredentials, OAuthLoginData, PaginationDirection, ReplyInfo, RoomId,
    ServerInfo, Session, SyncEvent, TimelineCommand, TimelineMessage, TimelinePatch,
    TimelineUpdate, VerificationEvent,
};
use crate::error::{AppError, Result};
use crate::ports::matrix::MatrixPort;
use crate::ports::media::MediaCache;

struct ActiveRoom {
    room_id: RoomId,
    timeline_tx: mpsc::UnboundedSender<TimelineUpdate>,
    messages: Vec<TimelineMessage>,
}

#[derive(Default)]
pub struct DemoMatrix {
    active: Mutex<Option<ActiveRoom>>,
    sent: AtomicU64,
}

impl DemoMatrix {
    fn append_own_message(&self, room_id: &RoomId, body: &str, in_reply_to: Option<&str>) {
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
        send_patch(&active.timeline_tx, TimelinePatch::PushBack(message));
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
        timeline_tx: mpsc::UnboundedSender<TimelineUpdate>,
        mut cmd_rx: mpsc::UnboundedReceiver<TimelineCommand>,
    ) -> Result<()> {
        let messages = data::messages(room_id);
        send_patch(&timeline_tx, TimelinePatch::Reset(messages.clone()));

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
                hit_end: true,
            };
            if timeline_tx.send(update).is_err() {
                break;
            }
        }

        Ok(())
    }

    async fn start_sync(&self, on_sync: Box<dyn Fn(SyncEvent) + Send + Sync>) -> Result<()> {
        on_sync(SyncEvent::Connected);
        on_sync(SyncEvent::Rooms(data::rooms()));
        on_sync(SyncEvent::Spaces(data::spaces()));
        Ok(())
    }

    async fn send_text(&self, room_id: &RoomId, body: &str) -> Result<()> {
        self.append_own_message(room_id, body, None);
        Ok(())
    }

    async fn send_reply(&self, room_id: &RoomId, body: &str, in_reply_to: &str) -> Result<()> {
        self.append_own_message(room_id, body, Some(in_reply_to));
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
        .find(|message| message.event_id.0 == event_id)
        .map(|message| ReplyInfo {
            sender: data::sender_label(message),
            preview: data::body_preview(&message.body),
        })
}

fn send_patch(timeline_tx: &mpsc::UnboundedSender<TimelineUpdate>, patch: TimelinePatch) {
    if let Err(e) = timeline_tx.send(TimelineUpdate::Patch(Box::new(patch))) {
        tracing::debug!("demo timeline receiver closed: {e}");
    }
}

fn unavailable(action: &str) -> AppError {
    AppError::Other(format!("{action} is not available in demo mode"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::MessageBody;

    fn patch(update: Option<TimelineUpdate>) -> Option<TimelinePatch> {
        match update {
            Some(TimelineUpdate::Patch(patch)) => Some(*patch),
            _ => None,
        }
    }

    async fn subscribed(
        matrix: &DemoMatrix,
        room: &RoomId,
    ) -> mpsc::UnboundedReceiver<TimelineUpdate> {
        let (timeline_tx, timeline_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<TimelineCommand>();
        drop(cmd_tx);
        assert!(
            matrix
                .subscribe_timeline(room, timeline_tx, cmd_rx)
                .await
                .is_ok()
        );
        timeline_rx
    }

    #[tokio::test]
    async fn resets_the_timeline_with_scripted_history() {
        let matrix = DemoMatrix::default();
        let room = RoomId::new("!rust:demo.local");
        let mut timeline_rx = subscribed(&matrix, &room).await;

        match patch(timeline_rx.recv().await) {
            Some(TimelinePatch::Reset(messages)) => {
                assert!(messages.len() > 1, "demo room should have scripted history");
            }
            _ => unreachable!("subscribing should reset the timeline"),
        }
    }

    #[tokio::test]
    async fn appends_sent_messages_and_resolves_replies() {
        let matrix = DemoMatrix::default();
        let room = RoomId::new("!rust:demo.local");
        let mut timeline_rx = subscribed(&matrix, &room).await;
        drop(timeline_rx.recv().await);

        assert!(
            matrix
                .send_reply(&room, "same", "demo-rust-1")
                .await
                .is_ok()
        );

        match patch(timeline_rx.recv().await) {
            Some(TimelinePatch::PushBack(message)) => {
                assert!(message.is_own);
                assert_eq!(message.body, MessageBody::Text("same".to_owned()));
                let reply = message.reply.unwrap_or(ReplyInfo {
                    sender: String::new(),
                    preview: String::new(),
                });
                assert_eq!(reply.sender, "Sarah Chen");
            }
            _ => unreachable!("sending should append the message to the active room"),
        }
    }
}
