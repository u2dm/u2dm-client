use std::collections::HashSet;
use std::mem;
use std::sync::Mutex as StdMutex;

use matrix_sdk::Room;
use matrix_sdk::config::RequestConfig;
use matrix_sdk::deserialized_responses::TimelineEvent;
use matrix_sdk::ruma::events::relation::RelationType;
use matrix_sdk::ruma::events::room::message::{
    FormattedBody, MessageFormat, MessageType, OriginalSyncRoomMessageEvent, Relation,
    ReplacementMetadata, RoomMessageEventContent, RoomMessageEventContentWithoutRelation,
};
use matrix_sdk::ruma::events::{
    AnyMessageLikeEventContent, AnySyncMessageLikeEvent, AnySyncTimelineEvent, Mentions,
    SyncMessageLikeEvent,
};
use matrix_sdk::ruma::html::{HtmlSanitizerMode, RemoveReplyFallback};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{EventId, OwnedEventId, OwnedUserId, UserId};
use matrix_sdk::send_queue::{LocalEcho, LocalEchoContent, SendHandle};
use matrix_sdk_ui::timeline::EventTimelineItem;
use tokio::time::timeout;

use super::{PRE_QUEUE_LOOKUP_WAIT, composed, local_transaction, queue, refuse_oversized};
use crate::adapters::body_emoji;
use crate::adapters::draft_size::MESSAGE_TYPE;
use crate::adapters::markdown::{self, Composed};
use crate::domain::emote::Emote;
use crate::domain::message::{EditKind, EditTarget, MessageEdit};
use crate::error::{AppError, Result};

const SANITIZER_MODE: HtmlSanitizerMode = HtmlSanitizerMode::Compat;

pub(super) async fn edit(room: &Room, edit: &MessageEdit) -> Result<()> {
    match &edit.target {
        EditTarget::Sent(event_id) => {
            let target = OwnedEventId::try_from(event_id.as_str())
                .map_err(|e| AppError::Other(e.to_string()))?;
            let content = replacement(room, &target, edit).await?;
            queue(room, content).await
        }
        EditTarget::Queued(local_id) => edit_queued(room, local_id, edit).await,
    }
}

async fn replacement(
    room: &Room,
    target: &EventId,
    edit: &MessageEdit,
) -> Result<AnyMessageLikeEventContent> {
    let queued = queued_version(room, target).await;
    let (event, related) = match cached_with_replacements(room, target).await {
        Some(cached) => cached,
        None => fetched_with_replacements(room, target).await?,
    };
    let original = original_message(event.raw())
        .filter(|original| original.sender == room.own_user_id())
        .ok_or_else(|| AppError::Other(format!("{target} is not a message of yours")))?;
    let shown = queued.unwrap_or_else(|| {
        latest_version(&original, &own_replacements(&related, room.own_user_id()))
    });
    let emotes = with_target_emotes(&edit.emotes, version_html(&shown.msgtype));
    let composed = composed(room, &edit.body, &emotes).await;
    let msgtype = revised(original.content.msgtype, &edit.body, &composed)
        .ok_or_else(|| AppError::Other(format!("{target} has no plain text or caption to edit")))?;
    let mentions = resolved_mentions(
        room,
        replied_to(original.content.relates_to.as_ref()),
        shown.mentions.as_ref(),
        &composed.mentions,
    )
    .await;
    let mut revised = RoomMessageEventContentWithoutRelation::new(msgtype);
    revised.mentions = Some(mentions);
    Ok(revised
        .make_replacement(ReplacementMetadata::new(target.to_owned(), shown.mentions))
        .into())
}

async fn queued_version(
    room: &Room,
    target: &EventId,
) -> Option<RoomMessageEventContentWithoutRelation> {
    let (echoes, _updates) = room.send_queue().subscribe().await.ok()?;
    echoes
        .iter()
        .rev()
        .filter_map(queued_message)
        .find_map(|(content, _handle)| match content.relates_to {
            Some(Relation::Replacement(replacement)) if replacement.event_id == target => {
                Some(replacement.new_content)
            }
            _ => None,
        })
}

async fn resolved_mentions(
    room: &Room,
    replied_to: Option<&OwnedEventId>,
    shown: Option<&Mentions>,
    tags: &Mentions,
) -> Mentions {
    let mut resolved = tags.clone();
    let Some(replied_to) = replied_to else {
        return resolved;
    };
    let Some(shown) = shown.filter(|shown| !shown.user_ids.is_subset(&tags.user_ids)) else {
        return resolved;
    };
    match replied_to_sender(room, replied_to).await {
        Ok(sender) if shown.user_ids.contains(&sender) => {
            resolved.user_ids.insert(sender);
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(
                %replied_to,
                "the edit keeps every mention of the version it replaces: {e}"
            );
            resolved.user_ids.extend(shown.user_ids.iter().cloned());
        }
    }
    resolved
}

fn replied_to<C>(relation: Option<&Relation<C>>) -> Option<&OwnedEventId> {
    match relation? {
        Relation::Reply(reply) => Some(&reply.in_reply_to.event_id),
        Relation::Thread(thread) => thread
            .in_reply_to
            .as_ref()
            .map(|in_reply_to| &in_reply_to.event_id),
        _ => None,
    }
}

async fn replied_to_sender(room: &Room, replied_to: &EventId) -> Result<OwnedUserId> {
    timeout(
        PRE_QUEUE_LOOKUP_WAIT,
        room.load_or_fetch_event(replied_to, Some(RequestConfig::short_retry())),
    )
    .await
    .map_err(|_| {
        AppError::Other(format!(
            "the replied-to event {replied_to} did not arrive in time"
        ))
    })?
    .map_err(|e| {
        AppError::Other(format!(
            "the replied-to event {replied_to} did not load: {e}"
        ))
    })?
    .sender()
    .ok_or_else(|| AppError::Other(format!("the replied-to event {replied_to} has no sender")))
}

async fn cached_with_replacements(
    room: &Room,
    target: &EventId,
) -> Option<(TimelineEvent, Vec<TimelineEvent>)> {
    let (cache, _drop_handles) = room.event_cache().await.ok()?;
    cache
        .find_event_with_relations(target, Some(vec![RelationType::Replacement]))
        .await
        .ok()
        .flatten()
}

async fn fetched_with_replacements(
    room: &Room,
    target: &EventId,
) -> Result<(TimelineEvent, Vec<TimelineEvent>)> {
    timeout(
        PRE_QUEUE_LOOKUP_WAIT,
        room.load_or_fetch_event_with_relations(
            target,
            Some(vec![RelationType::Replacement]),
            Some(RequestConfig::short_retry()),
        ),
    )
    .await
    .map_err(|_| AppError::Other(format!("the edited event {target} did not arrive in time")))?
    .map_err(|e| AppError::Other(e.to_string()))
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
        MessageType::File(mut file)
            if caption_is_composed(file.caption(), file.formatted_caption()) =>
        {
            recaption(
                &mut file.body,
                &mut file.filename,
                &mut file.formatted,
                composed,
            );
            Some(MessageType::File(file))
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

async fn edit_queued(room: &Room, local_id: &str, edit: &MessageEdit) -> Result<()> {
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
    let emotes = with_target_emotes(&edit.emotes, version_html(&queued.msgtype));
    let composed = &composed(room, &edit.body, &emotes).await;
    let msgtype = revised(queued.msgtype, &edit.body, composed).ok_or_else(|| {
        AppError::Other(format!("{local_id} has no plain text or caption to edit"))
    })?;
    let mentions = resolved_mentions(
        room,
        replied_to(queued.relates_to.as_ref()),
        queued.mentions.as_ref(),
        &composed.mentions,
    )
    .await;
    let mut revised = RoomMessageEventContentWithoutRelation::new(msgtype);
    revised.mentions = Some(mentions);
    let revised = revised.with_relation(queued.relates_to);
    refuse_oversized(room, &revised, MESSAGE_TYPE).await?;
    let replaced = if matches!(revised.msgtype, MessageType::Text(_)) {
        handle.edit(revised.into()).await
    } else {
        let (caption, formatted) = queued_caption(composed);
        handle
            .edit_media_caption(caption, formatted, revised.mentions)
            .await
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

fn own_replacements(
    related: &[TimelineEvent],
    own_user: &UserId,
) -> Vec<OriginalSyncRoomMessageEvent> {
    related
        .iter()
        .filter_map(|event| original_message(event.raw()))
        .filter(|event| {
            event.sender == own_user
                && matches!(event.content.relates_to, Some(Relation::Replacement(_)))
        })
        .collect()
}

fn latest_version(
    original: &OriginalSyncRoomMessageEvent,
    replacements: &[OriginalSyncRoomMessageEvent],
) -> RoomMessageEventContentWithoutRelation {
    replacements
        .iter()
        .max_by_key(|replacement| replacement.origin_server_ts)
        .and_then(|replacement| match &replacement.content.relates_to {
            Some(Relation::Replacement(edit)) => Some(edit.new_content.clone()),
            _ => None,
        })
        .unwrap_or_else(|| original.content.clone().into())
}

fn version_html(msgtype: &MessageType) -> Option<&str> {
    let formatted = match msgtype {
        MessageType::Text(text) => text.formatted.as_ref(),
        MessageType::Image(image) => image.formatted_caption(),
        MessageType::Video(video) => video.formatted_caption(),
        MessageType::Audio(audio) => audio.formatted_caption(),
        MessageType::File(file) => file.formatted_caption(),
        _ => None,
    }?;
    (formatted.format == MessageFormat::Html).then_some(formatted.body.as_str())
}

fn with_target_emotes(picked: &[Emote], target_html: Option<&str>) -> Vec<Emote> {
    let mut emotes = target_html
        .map(|html| body_emoji::emotes_in(&markdown::sanitized(html)))
        .unwrap_or_default();
    for emote in picked {
        if !emotes.iter().any(|known| known.token == emote.token) {
            emotes.push(emote.clone());
        }
    }
    emotes
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
    let emotes = version_html(&replacement.new_content.msgtype)
        .map(|html| body_emoji::emotes_in(&markdown::sanitized(html)))
        .unwrap_or_default();
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
        MessageType::File(file) => (
            EditKind::Caption,
            caption_form(file.caption(), file.formatted_caption()),
        ),
        _ => return None,
    };
    Some(MessageEdit {
        target: EditTarget::Sent(replacement.event_id.to_string()),
        kind,
        body,
        original: None,
        emotes,
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
