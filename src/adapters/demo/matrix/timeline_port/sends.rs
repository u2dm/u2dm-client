use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::sync::mpsc;

use crate::adapters::demo::identity::SendHold;
use crate::adapters::demo::matrix::timeline_tasks::spawn_seen;
use crate::adapters::demo::matrix::{ActiveRoom, DemoAuthed, unavailable, unavailable_if};
use crate::adapters::demo::{attachments, data, receipts, timeline};
use crate::domain::emote::Emote;
use crate::domain::image_pack::PackImage;
use crate::domain::media::OutgoingAttachment;
use crate::domain::message::{ReplyInfo, SendState, TimelineMessage};
use crate::domain::room::RoomId;
use crate::domain::timeline::{TimelinePatch, TimelineUpdate};
use crate::error::{AppError, Result};

#[derive(Clone, Copy)]
pub(super) enum Settle {
    Resend,
    Discard,
}

impl DemoAuthed {
    pub(super) fn send_state_in(&self, room_id: &RoomId) -> SendState {
        if self.holds_sends_in(room_id) {
            SendState::Failed
        } else {
            outgoing_send_state()
        }
    }

    pub(super) fn holds_sends_in(&self, room_id: &RoomId) -> bool {
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

    fn settle_own_send(&self, room_id: &RoomId, local_id: &str, settle: Settle) {
        let Ok(mut own_sends) = self.own_sends.lock() else {
            return;
        };
        let Some(sent) = own_sends.get_mut(room_id) else {
            return;
        };
        match settle {
            Settle::Resend => {
                if let Some(message) = sent
                    .iter_mut()
                    .find(|m| m.local_id.as_deref() == Some(local_id))
                {
                    mark_sent(message);
                }
            }
            Settle::Discard => sent.retain(|m| m.local_id.as_deref() != Some(local_id)),
        }
    }

    pub(super) fn append_own(
        &self,
        room_id: &RoomId,
        build: impl FnOnce(&ActiveRoom) -> TimelineMessage,
    ) -> Option<(mpsc::Sender<TimelineUpdate>, usize, TimelineMessage)> {
        self.with_active(room_id, |active| {
            let message = build(active);
            self.keep_own_send(room_id, &message);
            if !active.live {
                return None;
            }
            active.messages.push(message.clone());
            let index = active.messages.len() - 1;
            Some((active.timeline_tx.clone(), index, message))
        })
    }

    fn reply_in_room(&self, active: &ActiveRoom, event_id: &str) -> Option<ReplyInfo> {
        reply_info(&active.messages, event_id)
            .or_else(|| reply_info(&self.room_history(&active.room_id), event_id))
    }

    pub(super) async fn patch_queued_send(
        &self,
        room_id: &RoomId,
        local_id: &str,
        settle: Settle,
    ) -> Result<()> {
        let prepared = {
            let Ok(mut guard) = self.active.lock() else {
                return Err(unavailable("the demo timeline"));
            };
            let Some(active) = guard.as_mut() else {
                return Err(unavailable("the demo timeline"));
            };
            unavailable_if(&active.room_id != room_id, "the demo timeline")?;
            let index = active
                .messages
                .iter()
                .position(|m| m.local_id.as_deref() == Some(local_id))
                .ok_or_else(|| AppError::Other(format!("no queued send for {local_id}")))?;
            let patch = match settle {
                Settle::Resend => {
                    let Some(message) = active.messages.get_mut(index) else {
                        return Err(unavailable("the demo timeline"));
                    };
                    mark_sent(message);
                    TimelinePatch::Set {
                        index,
                        message: message.clone(),
                    }
                }
                Settle::Discard => {
                    active.messages.remove(index);
                    TimelinePatch::Remove { index }
                }
            };
            self.settle_own_send(room_id, local_id, settle);
            (active.timeline_tx.clone(), patch)
        };
        let (timeline_tx, patch) = prepared;
        send_patch(&timeline_tx, patch).await;
        Ok(())
    }

    pub(super) async fn append_own_message(
        &self,
        room_id: &RoomId,
        body: &str,
        emotes: &[Emote],
        in_reply_to: Option<&str>,
    ) {
        let Some((timeline_tx, _, message)) = self.append_own(room_id, |active| {
            let reply = in_reply_to.and_then(|event_id| self.reply_in_room(active, event_id));
            data::own_message(
                self.sent.fetch_add(1, Ordering::Relaxed),
                body,
                emotes,
                reply,
                self.send_state_in(room_id),
            )
        }) else {
            return;
        };

        let seen = receipts::scenario().member_sees_sends && message.event_id.is_some();
        let unique_id = message.unique_id.clone();
        send_patch(&timeline_tx, TimelinePatch::PushBack(message)).await;
        if seen {
            spawn_seen(Arc::clone(&self.active), unique_id);
        }
    }

    pub(in crate::adapters::demo::matrix) async fn append_own_sticker(
        &self,
        room_id: &RoomId,
        image: &PackImage,
        in_reply_to: Option<&str>,
    ) {
        let Some((timeline_tx, _, message)) = self.append_own(room_id, |active| {
            let reply = in_reply_to.and_then(|event_id| self.reply_in_room(active, event_id));
            data::own_sticker(self.sent.fetch_add(1, Ordering::Relaxed), image, reply)
        }) else {
            return;
        };

        send_patch(&timeline_tx, TimelinePatch::PushBack(message)).await;
    }

    pub(in crate::adapters::demo::matrix) async fn post_own_row(
        &self,
        room_id: &RoomId,
        message: TimelineMessage,
    ) {
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

    pub(super) fn timeline_sender(&self, room_id: &RoomId) -> Option<mpsc::Sender<TimelineUpdate>> {
        let guard = self.active.lock().ok()?;
        let active = guard.as_ref()?;
        (&active.room_id == room_id).then(|| active.timeline_tx.clone())
    }

    pub(super) async fn append_own_attachment(
        &self,
        room_id: &RoomId,
        attachment: &OutgoingAttachment,
        opening: SendState,
    ) -> Option<(usize, TimelineMessage)> {
        let (timeline_tx, index, settled) = self.append_own(room_id, |active| {
            let reply = attachment
                .reply_to
                .as_deref()
                .and_then(|event_id| self.reply_in_room(active, event_id));
            let settled =
                data::own_attachment(self.sent.fetch_add(1, Ordering::Relaxed), attachment, reply);
            remember_sent_media(&settled, attachment);
            settled
        })?;

        let mut opening_echo = settled.clone();
        opening_echo.send_state = opening;
        send_patch(&timeline_tx, TimelinePatch::PushBack(opening_echo)).await;
        Some((index, settled))
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

pub(in crate::adapters::demo::matrix) async fn send_patch(
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
    patch: TimelinePatch,
) {
    send_update(timeline_tx, TimelineUpdate::patch(patch)).await;
}

pub(in crate::adapters::demo::matrix) async fn send_update(
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
    update: TimelineUpdate,
) {
    if let Err(e) = timeline_tx.send(update).await {
        tracing::debug!("demo timeline receiver closed: {e}");
    }
}
