use std::sync::Arc;

use matrix_sdk::deserialized_responses::TimelineEvent;
use matrix_sdk::ruma::events::relation::RelationType;
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::ruma::events::room::message::{MessageType, Relation};
use matrix_sdk::ruma::events::sticker::StickerEventContent;
use matrix_sdk::ruma::events::{
    AnySyncMessageLikeEvent, AnySyncTimelineEvent, SyncMessageLikeEvent,
};
use matrix_sdk::ruma::{EventId, OwnedEventId};
use matrix_sdk::{Room, check_validity_of_replacement_events};

use super::MediaLane;
use crate::domain::media::{MediaFailure, MediaResult};

type EventWithEdits = (TimelineEvent, Vec<TimelineEvent>);

pub(crate) struct EventMedia {
    pub(super) file: MediaSource,
    pub(super) thumbnail: Option<MediaSource>,
}

impl EventMedia {
    fn file_only(file: MediaSource) -> Self {
        Self {
            file,
            thumbnail: None,
        }
    }

    pub(crate) fn of_message(msgtype: &MessageType) -> Option<Self> {
        match msgtype {
            MessageType::Image(image) => Some(Self {
                file: image.source.clone(),
                thumbnail: image
                    .info
                    .as_ref()
                    .and_then(|info| info.thumbnail_source.clone()),
            }),
            MessageType::Video(video) => Some(Self {
                file: video.source.clone(),
                thumbnail: video
                    .info
                    .as_ref()
                    .and_then(|info| info.thumbnail_source.clone()),
            }),
            MessageType::Audio(audio) => Some(Self::file_only(audio.source.clone())),
            MessageType::File(file) => Some(Self::file_only(file.source.clone())),
            _ => None,
        }
    }

    pub(crate) fn of_sticker(sticker: &StickerEventContent) -> Self {
        Self::file_only(MediaSource::from(sticker.source.clone()))
    }
}

pub(super) async fn resolve(
    room: &Room,
    event_id: &str,
    lane: MediaLane,
) -> MediaResult<MediaSource> {
    let event_id = OwnedEventId::try_from(event_id).map_err(|_| MediaFailure::NoSource)?;
    let (original, edits) = match cached_with_edits(room, &event_id).await {
        Some(cached) => cached,
        None => fetch_with_edits(room, &event_id).await?,
    };
    latest_media(&original, &edits)
        .and_then(|media| lane.pick(media))
        .ok_or(MediaFailure::NoSource)
}

async fn cached_with_edits(room: &Room, event_id: &EventId) -> Option<EventWithEdits> {
    let (cache, _drop_handles) = room.event_cache().await.ok()?;
    cache
        .find_event_with_relations(event_id, Some(vec![RelationType::Replacement]))
        .await
        .ok()
        .flatten()
}

async fn fetch_with_edits(room: &Room, event_id: &EventId) -> MediaResult<EventWithEdits> {
    room.load_or_fetch_event_with_relations(event_id, Some(vec![RelationType::Replacement]), None)
        .await
        .map_err(|e| {
            tracing::debug!(%event_id, "failed to fetch the event behind a media download: {e}");
            MediaFailure::Download
        })
}

fn latest_media(original: &TimelineEvent, edits: &[TimelineEvent]) -> Option<EventMedia> {
    match edits
        .iter()
        .rev()
        .find_map(|edit| replacement_msgtype(original, edit))
    {
        Some(msgtype) => EventMedia::of_message(&msgtype),
        None => original_media(original),
    }
}

fn replacement_msgtype(original: &TimelineEvent, edit: &TimelineEvent) -> Option<MessageType> {
    check_validity_of_replacement_events(
        original.raw(),
        original.encryption_info().map(Arc::as_ref),
        edit.raw(),
        edit.encryption_info().map(Arc::as_ref),
    )
    .ok()?;
    let AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomMessage(
        SyncMessageLikeEvent::Original(message),
    )) = edit.raw().deserialize().ok()?
    else {
        return None;
    };
    let Relation::Replacement(replacement) = message.content.relates_to? else {
        return None;
    };
    Some(replacement.new_content.msgtype)
}

fn original_media(event: &TimelineEvent) -> Option<EventMedia> {
    match event.raw().deserialize().ok()? {
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomMessage(
            SyncMessageLikeEvent::Original(message),
        )) => EventMedia::of_message(&message.content.msgtype),
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::Sticker(
            SyncMessageLikeEvent::Original(sticker),
        )) => Some(EventMedia::of_sticker(&sticker.content)),
        _ => None,
    }
}
