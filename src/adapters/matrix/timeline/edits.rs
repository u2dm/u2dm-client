use std::collections::HashSet;
use std::mem;
use std::sync::Mutex as StdMutex;

use matrix_sdk::Room;
use matrix_sdk::ruma::events::room::message::{
    FormattedBody, MessageFormat, MessageType, OriginalSyncRoomMessageEvent, Relation,
    ReplacementMetadata, RoomMessageEventContent, RoomMessageEventContentWithoutRelation,
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

use super::{PRE_QUEUE_LOOKUP_WAIT, composed, local_transaction, queue};
use crate::adapters::markdown::{self, Composed};
use crate::domain::message::{EditKind, EditTarget, MessageEdit};
use crate::error::{AppError, Result};

const SANITIZER_MODE: HtmlSanitizerMode = HtmlSanitizerMode::Compat;

pub(super) async fn edit(room: &Room, edit: &MessageEdit) -> Result<()> {
    let composed = composed(room, &edit.body).await;
    match &edit.target {
        EditTarget::Sent(event_id) => {
            let target = OwnedEventId::try_from(event_id.as_str())
                .map_err(|e| AppError::Other(e.to_string()))?;
            let content = replacement(room, &target, &edit.body, &composed).await?;
            queue(room, content).await
        }
        EditTarget::Queued(local_id) => edit_queued(room, local_id, &edit.body, &composed).await,
    }
}

async fn replacement(
    room: &Room,
    target: &EventId,
    text: &str,
    composed: &Composed,
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
    let previous = original.content.mentions;
    let msgtype = revised(original.content.msgtype, text, composed)
        .ok_or_else(|| AppError::Other(format!("{target} has no plain text or caption to edit")))?;
    let mut revised = RoomMessageEventContentWithoutRelation::new(msgtype);
    revised.mentions = Some(markdown::merged(previous.as_ref(), &composed.mentions));
    Ok(revised
        .make_replacement(ReplacementMetadata::new(target.to_owned(), previous))
        .into())
}

fn revised(msgtype: MessageType, text: &str, composed: &Composed) -> Option<MessageType> {
    match msgtype {
        MessageType::Text(original)
            if !text.trim().is_empty()
                && is_composed(&original.body, original.formatted.as_ref()) =>
        {
            Some(MessageType::Text(composed.content()))
        }
        MessageType::Image(mut image)
            if caption_is_composed(image.caption(), image.formatted_caption()) =>
        {
            recaption(
                &mut image.body,
                &mut image.filename,
                &mut image.formatted,
                composed,
            );
            Some(MessageType::Image(image))
        }
        MessageType::Video(mut video)
            if caption_is_composed(video.caption(), video.formatted_caption()) =>
        {
            recaption(
                &mut video.body,
                &mut video.filename,
                &mut video.formatted,
                composed,
            );
            Some(MessageType::Video(video))
        }
        MessageType::Audio(mut audio)
            if caption_is_composed(audio.caption(), audio.formatted_caption()) =>
        {
            recaption(
                &mut audio.body,
                &mut audio.filename,
                &mut audio.formatted,
                composed,
            );
            Some(MessageType::Audio(audio))
        }
        _ => None,
    }
}

fn is_composed(body: &str, formatted: Option<&FormattedBody>) -> bool {
    match formatted {
        None => true,
        Some(formatted) if formatted.format == MessageFormat::Html => {
            markdown::composer_text(body, Some(&markdown::sanitized(&formatted.body))).is_some()
        }
        Some(_) => false,
    }
}

fn caption_is_composed(caption: Option<&str>, formatted: Option<&FormattedBody>) -> bool {
    caption.is_none_or(|caption| is_composed(caption, formatted))
}

fn composer_form(body: &str, formatted: Option<&FormattedBody>) -> String {
    let html = formatted
        .filter(|formatted| formatted.format == MessageFormat::Html)
        .map(|formatted| markdown::sanitized(&formatted.body));
    markdown::composer_text(body, html.as_deref()).unwrap_or_else(|| body.to_owned())
}

fn caption_form(caption: Option<&str>, formatted: Option<&FormattedBody>) -> String {
    caption.map_or_else(String::new, |caption| composer_form(caption, formatted))
}

fn recaption(
    body: &mut String,
    filename: &mut Option<String>,
    formatted: &mut Option<FormattedBody>,
    composed: &Composed,
) {
    let name = filename.take().unwrap_or_else(|| mem::take(body));
    if composed.markdown.is_empty() {
        *body = name;
        *formatted = None;
    } else {
        composed.markdown.clone_into(body);
        *formatted = composed.html.clone().map(FormattedBody::html);
        *filename = Some(name);
    }
}

fn queued_caption(composed: &Composed) -> (Option<String>, Option<FormattedBody>) {
    if composed.markdown.is_empty() {
        return (None, None);
    }
    (
        Some(composed.markdown.clone()),
        composed.html.clone().map(FormattedBody::html),
    )
}

fn original_message(raw: &Raw<AnySyncTimelineEvent>) -> Option<OriginalSyncRoomMessageEvent> {
    match raw.deserialize().ok()? {
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomMessage(
            SyncMessageLikeEvent::Original(original),
        )) => Some(original),
        _ => None,
    }
}

async fn edit_queued(room: &Room, local_id: &str, text: &str, composed: &Composed) -> Result<()> {
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
    let replaced = match revised(queued.msgtype, text, composed) {
        Some(msgtype @ MessageType::Text(_)) => {
            let mut revised = RoomMessageEventContentWithoutRelation::new(msgtype);
            revised.mentions = Some(markdown::merged(
                queued.mentions.as_ref(),
                &composed.mentions,
            ));
            handle
                .edit(revised.with_relation(queued.relates_to).into())
                .await
        }
        Some(_) => {
            let (caption, formatted) = queued_caption(composed);
            let mentions = markdown::merged(queued.mentions.as_ref(), &composed.mentions);
            handle
                .edit_media_caption(caption, formatted, Some(mentions))
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
        MessageType::Text(text) => (
            EditKind::Message,
            composer_form(&text.body, text.formatted.as_ref()),
        ),
        MessageType::Image(image) => (
            EditKind::Caption,
            caption_form(image.caption(), image.formatted_caption()),
        ),
        MessageType::Video(video) => (
            EditKind::Caption,
            caption_form(video.caption(), video.formatted_caption()),
        ),
        MessageType::Audio(audio) => (
            EditKind::Caption,
            caption_form(audio.caption(), audio.formatted_caption()),
        ),
        _ => return None,
    };
    Some(MessageEdit {
        target: EditTarget::Sent(replacement.event_id.to_string()),
        kind,
        body,
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

    pub(super) fn original_content(
        &self,
        event: &EventTimelineItem,
    ) -> Option<RoomMessageEventContent> {
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
        Some(content)
    }
}
