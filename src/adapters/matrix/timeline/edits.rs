use std::collections::HashSet;
use std::mem;
use std::sync::Mutex as StdMutex;

use matrix_sdk::Room;
use matrix_sdk::ruma::events::room::message::{
    MessageType, OriginalSyncRoomMessageEvent, Relation, ReplacementMetadata,
    RoomMessageEventContent, RoomMessageEventContentWithoutRelation, TextMessageEventContent,
};
use matrix_sdk::ruma::events::{
    AnyMessageLikeEventContent, AnySyncMessageLikeEvent, AnySyncTimelineEvent, SyncMessageLikeEvent,
};
use matrix_sdk::ruma::html::{HtmlSanitizerMode, RemoveReplyFallback};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{EventId, OwnedEventId};
use matrix_sdk::send_queue::{LocalEcho, LocalEchoContent, SendHandle};
use matrix_sdk_ui::timeline::EventTimelineItem;
use tokio::time::timeout;

use super::{PRE_QUEUE_LOOKUP_WAIT, local_transaction, queue};
use crate::domain::message::{EditKind, EditTarget, MessageEdit};
use crate::error::{AppError, Result};

const SANITIZER_MODE: HtmlSanitizerMode = HtmlSanitizerMode::Compat;

pub(super) async fn edit(room: &Room, edit: &MessageEdit) -> Result<()> {
    match &edit.target {
        EditTarget::Sent(event_id) => {
            let target = OwnedEventId::try_from(event_id.as_str())
                .map_err(|e| AppError::Other(e.to_string()))?;
            let content = replacement(room, &target, &edit.body).await?;
            queue(room, content).await
        }
        EditTarget::Queued(local_id) => edit_queued(room, local_id, &edit.body).await,
    }
}

async fn replacement(
    room: &Room,
    target: &EventId,
    text: &str,
) -> Result<AnyMessageLikeEventContent> {
    let event = timeout(
        PRE_QUEUE_LOOKUP_WAIT,
        room.load_or_fetch_event(target, None),
    )
    .await
    .map_err(|_| AppError::Other(format!("the edited event {target} did not arrive in time")))?
    .map_err(|e| AppError::Other(e.to_string()))?;
    let original = original_message(event.raw())
        .filter(|original| original.sender == room.own_user_id())
        .ok_or_else(|| AppError::Other(format!("{target} is not a message of yours")))?;
    let mentions = original.content.mentions;
    let msgtype = revised(original.content.msgtype, text)
        .ok_or_else(|| AppError::Other(format!("{target} has no plain text or caption to edit")))?;
    let mut revised = RoomMessageEventContentWithoutRelation::new(msgtype);
    revised.mentions.clone_from(&mentions);
    Ok(revised
        .make_replacement(ReplacementMetadata::new(target.to_owned(), mentions))
        .into())
}

fn revised(msgtype: MessageType, text: &str) -> Option<MessageType> {
    match msgtype {
        MessageType::Text(TextMessageEventContent {
            formatted: None, ..
        }) if !text.trim().is_empty() => Some(MessageType::text_plain(text)),
        MessageType::Image(mut image) if image.formatted_caption().is_none() => {
            image.formatted = None;
            recaption(&mut image.body, &mut image.filename, text);
            Some(MessageType::Image(image))
        }
        MessageType::Video(mut video) if video.formatted_caption().is_none() => {
            video.formatted = None;
            recaption(&mut video.body, &mut video.filename, text);
            Some(MessageType::Video(video))
        }
        MessageType::Audio(mut audio) if audio.formatted_caption().is_none() => {
            audio.formatted = None;
            recaption(&mut audio.body, &mut audio.filename, text);
            Some(MessageType::Audio(audio))
        }
        _ => None,
    }
}

fn recaption(body: &mut String, filename: &mut Option<String>, caption: &str) {
    let name = filename.take().unwrap_or_else(|| mem::take(body));
    if caption.is_empty() {
        *body = name;
    } else {
        caption.clone_into(body);
        *filename = Some(name);
    }
}

fn caption(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

fn original_message(raw: &Raw<AnySyncTimelineEvent>) -> Option<OriginalSyncRoomMessageEvent> {
    match raw.deserialize().ok()? {
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomMessage(
            SyncMessageLikeEvent::Original(original),
        )) => Some(original),
        _ => None,
    }
}

async fn edit_queued(room: &Room, local_id: &str, text: &str) -> Result<()> {
    let transaction = local_transaction(local_id);
    let (echoes, _updates) = room
        .send_queue()
        .subscribe()
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;
    let (queued, handle) = echoes
        .iter()
        .filter(|echo| echo.transaction_id.as_str() == transaction)
        .find_map(queued_message)
        .ok_or_else(|| {
            AppError::Other(format!(
                "{local_id} is no longer a message waiting in the send queue"
            ))
        })?;
    let replaced = match revised(queued.msgtype, text) {
        Some(msgtype @ MessageType::Text(_)) => {
            let mut revised = RoomMessageEventContentWithoutRelation::new(msgtype);
            revised.mentions = queued.mentions;
            handle
                .edit(revised.with_relation(queued.relates_to).into())
                .await
        }
        Some(_) => {
            handle
                .edit_media_caption(caption(text), None, queued.mentions)
                .await
        }
        None => {
            return Err(AppError::Other(format!(
                "{local_id} has no plain text or caption to edit"
            )));
        }
    }
    .map_err(|e| AppError::Other(e.to_string()))?;
    if replaced {
        Ok(())
    } else {
        Err(AppError::Other(format!(
            "{local_id} was sent before its edit reached the send queue"
        )))
    }
}

fn queued_message(echo: &LocalEcho) -> Option<(RoomMessageEventContent, &SendHandle)> {
    let LocalEchoContent::Event {
        serialized_event,
        send_handle,
        send_error: None,
    } = &echo.content
    else {
        return None;
    };
    let AnyMessageLikeEventContent::RoomMessage(content) = serialized_event.deserialize().ok()?
    else {
        return None;
    };
    Some((content, send_handle))
}

pub(super) fn replaced_text(content: &Raw<AnyMessageLikeEventContent>) -> Option<MessageEdit> {
    let content = content
        .deserialize_as_unchecked::<RoomMessageEventContent>()
        .ok()?;
    let Some(Relation::Replacement(replacement)) = content.relates_to else {
        return None;
    };
    let (kind, body) = match &replacement.new_content.msgtype {
        MessageType::Text(text) => (EditKind::Message, text.body.as_str()),
        MessageType::Image(image) => (EditKind::Caption, image.caption().unwrap_or_default()),
        MessageType::Video(video) => (EditKind::Caption, video.caption().unwrap_or_default()),
        MessageType::Audio(audio) => (EditKind::Caption, audio.caption().unwrap_or_default()),
        _ => return None,
    };
    Some(MessageEdit {
        target: EditTarget::Sent(replacement.event_id.to_string()),
        kind,
        body: body.to_owned(),
        original: None,
    })
}

#[derive(Default)]
pub(in crate::adapters::matrix) struct DiscardedEdits {
    targets: StdMutex<HashSet<OwnedEventId>>,
}

impl DiscardedEdits {
    pub(super) fn record(&self, target: &str) {
        let Ok(target) = OwnedEventId::try_from(target) else {
            return;
        };
        if let Ok(mut targets) = self.targets.lock() {
            targets.insert(target);
        }
    }

    pub(super) fn original_msgtype(&self, event: &EventTimelineItem) -> Option<MessageType> {
        let event_id = event.event_id()?;
        let discarded = self
            .targets
            .lock()
            .is_ok_and(|targets| targets.contains(event_id));
        if !discarded || event.latest_edit_json().is_some() || event.edit_send_state().is_some() {
            return None;
        }
        let mut content = original_message(event.original_json()?)?.content;
        content.sanitize(SANITIZER_MODE, RemoveReplyFallback::Yes);
        Some(content.msgtype)
    }
}
