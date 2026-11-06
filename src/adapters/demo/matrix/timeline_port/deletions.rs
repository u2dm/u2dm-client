use std::collections::HashSet;

use tokio::sync::mpsc;
use tokio::time::sleep;

use super::sends::send_patch;
use crate::adapters::demo::matrix::{DemoAuthed, SharedActiveRoom};
use crate::adapters::demo::message_menu;
use crate::domain::message::{MessagePreviewKind, RichText, TimelineMessage};
use crate::domain::room::RoomId;
use crate::domain::timeline::{TimelinePatch, TimelineUpdate};

impl DemoAuthed {
    pub(super) fn remove_from_window(
        &self,
        room_id: &RoomId,
        event_id: &str,
        restamp_replies: bool,
    ) -> Option<RemovedMessage> {
        self.with_active(room_id, |active| {
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
        })
    }
}

pub(super) struct RemovedMessage {
    pub(super) timeline_tx: mpsc::Sender<TimelineUpdate>,
    offset: usize,
    pub(super) row: usize,
    message: TimelineMessage,
    pub(super) replies: Vec<(usize, TimelineMessage)>,
}

pub(super) fn stamp_deleted_reply(
    message: &mut TimelineMessage,
    deleted: &HashSet<String>,
) -> bool {
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

pub(super) fn spawn_deletion_refusal(active: SharedActiveRoom, removed: RemovedMessage) {
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
