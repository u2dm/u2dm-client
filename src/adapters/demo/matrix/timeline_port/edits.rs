use std::collections::BTreeMap;
use std::mem;

use tokio::sync::mpsc;
use tokio::time::sleep;

use super::sends::send_patch;
use crate::adapters::body_emoji;
use crate::adapters::demo::matrix::{DemoAuthed, SharedActiveRoom};
use crate::adapters::demo::{data, media, timeline};
use crate::domain::emote::Emote;
use crate::domain::message::{
    CustomEmoji, EditTarget, MessageBody, MessageEdit, RichText, TimelineMessage,
};
use crate::domain::room::RoomId;
use crate::domain::timeline::{TimelinePatch, TimelineUpdate};

pub(super) struct Revision {
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

pub(super) struct RevisedText {
    pub(super) timeline_tx: mpsc::Sender<TimelineUpdate>,
    pub(super) row: usize,
    pub(super) previous: Revision,
    pub(super) message: TimelineMessage,
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

impl DemoAuthed {
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

    pub(super) fn revise_own_text(
        &self,
        room_id: &RoomId,
        edit: &MessageEdit,
    ) -> Option<RevisedText> {
        self.with_active(room_id, |active| {
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
        })
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

pub(super) fn spawn_edit_refusal(
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
