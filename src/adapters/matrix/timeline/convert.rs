use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::ruma::events::room::message::{
    FileMessageEventContent, ImageMessageEventContent, MessageType,
};
use matrix_sdk_ui::timeline::{EventTimelineItem, TimelineDetails, TimelineItem};

use crate::domain::models::{EventId, FileMeta, ImageMeta, MessageBody, TimelineMessage};

fn extract_sender_profile(event: &EventTimelineItem) -> (Option<String>, Option<String>) {
    match event.sender_profile() {
        TimelineDetails::Ready(profile) => (
            profile.display_name.clone(),
            profile.avatar_url.as_ref().map(ToString::to_string),
        ),
        _ => (None, None),
    }
}

fn build_utd_message(
    unique_id: String,
    event: &EventTimelineItem,
    event_id_str: String,
    own_user_id: Option<&str>,
) -> TimelineMessage {
    let (sender_display_name, sender_avatar_url) = extract_sender_profile(event);
    let ts: u64 = event.timestamp().0.into();
    let sender_str = event.sender().to_string();
    let is_own = own_user_id.is_some_and(|uid| uid == sender_str);
    TimelineMessage {
        unique_id,
        event_id: EventId(event_id_str),
        sender: sender_str,
        sender_display_name,
        sender_avatar_url,
        sender_avatar_path: None,
        body: MessageBody::UnableToDecrypt,
        timestamp: ts,
        is_own,
    }
}

fn extract_image_body(
    image: &ImageMessageEventContent,
    event_id_str: &str,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
) -> MessageBody {
    if let Ok(mut sources) = media_sources.lock() {
        sources.insert(event_id_str.to_owned(), image.source.clone());
        if let Some(info) = &image.info
            && let Some(ref thumb_source) = info.thumbnail_source
        {
            sources.insert(format!("{event_id_str}:thumb"), thumb_source.clone());
        }
    }
    #[allow(clippy::cast_possible_truncation)]
    let (width, height, mimetype) = image.info.as_ref().map_or((None, None, None), |info| {
        let w = info.width.map(|v| {
            let n: u64 = v.into();
            n as u32
        });
        let h = info.height.map(|v| {
            let n: u64 = v.into();
            n as u32
        });
        (w, h, info.mimetype.clone())
    });
    MessageBody::Image {
        caption: image.caption().map(String::from),
        meta: ImageMeta {
            width,
            height,
            mimetype,
            thumbnail_path: None,
        },
    }
}

fn extract_file_body(
    file: &FileMessageEventContent,
    event_id_str: &str,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
) -> MessageBody {
    if let Ok(mut sources) = media_sources.lock() {
        sources.insert(event_id_str.to_owned(), file.source.clone());
    }
    let (mimetype, size) = file.info.as_ref().map_or((None, None), |info| {
        (info.mimetype.clone(), info.size.map(Into::into))
    });
    MessageBody::File {
        meta: FileMeta {
            filename: file.filename.clone().unwrap_or_else(|| file.body.clone()),
            mimetype,
            size,
        },
    }
}

fn message_type_to_body(
    msgtype: &MessageType,
    event_id_str: &str,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
) -> MessageBody {
    match msgtype {
        MessageType::Text(t) => MessageBody::Text(t.body.clone()),
        MessageType::Notice(n) => MessageBody::Notice(n.body.clone()),
        MessageType::Emote(e) => MessageBody::Emote(e.body.clone()),
        MessageType::Image(i) => extract_image_body(i, event_id_str, media_sources),
        MessageType::File(f) => extract_file_body(f, event_id_str, media_sources),
        other => MessageBody::Unsupported {
            kind: other.msgtype().to_string(),
            fallback: other.body().to_string(),
        },
    }
}

pub(super) fn convert_timeline_item(
    item: &TimelineItem,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
    own_user_id: Option<&str>,
) -> Option<TimelineMessage> {
    let event = item.as_event()?;
    let unique_id = item.unique_id().0.clone();
    convert_event_item_with_uid(unique_id, event, media_sources, own_user_id)
}

pub(super) fn convert_event_item_with_uid(
    unique_id: String,
    event: &EventTimelineItem,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
    own_user_id: Option<&str>,
) -> Option<TimelineMessage> {
    let event_id_str = event
        .event_id()
        .map(ToString::to_string)
        .unwrap_or_default();

    let content = event.content();

    let Some(message) = content.as_message() else {
        if content.as_unable_to_decrypt().is_some() {
            return Some(build_utd_message(
                unique_id,
                event,
                event_id_str,
                own_user_id,
            ));
        }
        tracing::debug!(
            event_id = event_id_str,
            sender = %event.sender(),
            "skipping non-message event"
        );
        return None;
    };

    let body = message_type_to_body(message.msgtype(), &event_id_str, media_sources);
    let (sender_display_name, sender_avatar_url) = extract_sender_profile(event);
    let ts: u64 = event.timestamp().0.into();
    let sender_str = event.sender().to_string();
    let is_own = own_user_id.is_some_and(|uid| uid == sender_str);

    Some(TimelineMessage {
        unique_id,
        event_id: EventId(event_id_str),
        sender: sender_str,
        sender_display_name,
        sender_avatar_url,
        sender_avatar_path: None,
        body,
        timestamp: ts,
        is_own,
    })
}
