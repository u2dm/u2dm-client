use slint::{Image, SharedString};

use super::decode::{
    AvatarSlot, load_avatar_async, load_thumbnail, peek_avatar, peek_thumbnail, record_avatar_need,
    record_media_need,
};
use super::present::{
    MessageKind, PreviewKind, ServiceKind, avatar_color_index, avatar_initials, message_body_text,
    message_kind, message_sender_label, message_timestamp_label, preview_kind, pronoun_labels,
    room_activity_label, sender_initial, service_kind, service_target, unsupported_kind,
};
use crate::domain::models::{
    EnrichmentDelta, MessageBody, Room, Space, ThumbnailOutcome, TimelineMessage,
};
use crate::ports::media::MediaCache;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MediaState {
    Idle,
    Ready,
    Failed,
}

#[cfg(feature = "interpreted")]
impl MediaState {
    pub fn slint(self) -> (&'static str, &'static str) {
        match self {
            Self::Idle => ("MediaState", "idle"),
            Self::Ready => ("MediaState", "ready"),
            Self::Failed => ("MediaState", "failed"),
        }
    }
}

#[allow(clippy::struct_excessive_bools)]
pub struct MessageDto {
    pub unique_id: SharedString,
    pub sender: SharedString,
    pub pronouns: Vec<SharedString>,
    pub body: SharedString,
    pub timestamp: SharedString,
    pub message_type: MessageKind,
    pub preview_kind: PreviewKind,
    pub unsupported_kind: SharedString,
    pub event_id: SharedString,
    pub sender_initial: SharedString,
    pub color_index: i32,
    pub is_own: bool,
    pub edited: bool,
    pub has_reply: bool,
    pub reply_sender: SharedString,
    pub reply_kind: PreviewKind,
    pub reply_body: SharedString,
    pub service_kind: ServiceKind,
    pub service_target: SharedString,
    pub image_width: i32,
    pub image_height: i32,
    pub thumbnail: Option<Image>,
    pub media_state: MediaState,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
}

pub struct RoomDto {
    pub id: SharedString,
    pub name: SharedString,
    pub initial: SharedString,
    pub color_index: i32,
    pub members: i32,
    pub unread: i32,
    pub mentions: i32,
    pub last_message_sender: SharedString,
    pub last_message_kind: PreviewKind,
    pub last_message_body: SharedString,
    pub last_message_service_kind: ServiceKind,
    pub last_message_service_target: SharedString,
    pub last_message_is_own: bool,
    pub last_message_edited: bool,
    pub last_message_time: SharedString,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
}

pub struct SpaceDto {
    pub id: SharedString,
    pub name: SharedString,
    pub unread: i32,
    pub mentions: i32,
    pub initial: SharedString,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
}

pub enum ThumbUpdate {
    Unchanged,
    Failed,
    Ready(Image),
}

pub struct EnrichUpdate {
    pub thumbnail: ThumbUpdate,
    pub avatar: Option<Image>,
    pub pronouns: Option<Vec<SharedString>>,
}

fn count(value: u64) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

pub fn message_to_dto(m: &TimelineMessage, media: &dyn MediaCache) -> MessageDto {
    let sender_label = message_sender_label(m);
    let mut dto = MessageDto {
        unique_id: SharedString::from(&m.unique_id),
        sender: SharedString::from(sender_label),
        pronouns: pronoun_labels(&m.sender_pronouns)
            .into_iter()
            .map(SharedString::from)
            .collect(),
        body: SharedString::from(message_body_text(&m.body)),
        timestamp: SharedString::from(&message_timestamp_label(m.timestamp)),
        message_type: message_kind(&m.body),
        preview_kind: preview_kind(m.body.preview_kind()),
        unsupported_kind: SharedString::from(unsupported_kind(&m.body)),
        event_id: SharedString::from(m.event_id.as_ref().map_or("", |e| e.0.as_str())),
        sender_initial: SharedString::from(avatar_initials(sender_label)),
        color_index: avatar_color_index(&m.sender),
        is_own: m.is_own,
        edited: m.edited,
        has_reply: m.reply.is_some(),
        reply_sender: SharedString::from(m.reply.as_ref().map_or("", |r| r.sender.as_str())),
        reply_kind: m
            .reply
            .as_ref()
            .map_or(PreviewKind::None, |r| preview_kind(r.kind)),
        reply_body: SharedString::from(m.reply.as_ref().map_or("", |r| r.body.as_str())),
        service_kind: m.body.service().map_or(ServiceKind::None, service_kind),
        service_target: SharedString::from(m.body.service().map_or("", service_target)),
        image_width: 0,
        image_height: 0,
        thumbnail: None,
        media_state: MediaState::Idle,
        avatar: None,
        has_avatar: false,
    };

    let mut thumbnail_path = None;
    if let MessageBody::Image { meta, .. } = &m.body {
        dto.image_width = meta.width.unwrap_or(0).cast_signed();
        dto.image_height = meta.height.unwrap_or(0).cast_signed();
        if let Some(event_id) = m.event_id.as_ref() {
            if let Some(path) = media.thumbnail_path(&event_id.0) {
                if let Some(img) = peek_thumbnail(&path) {
                    dto.thumbnail = Some(img);
                    dto.media_state = MediaState::Ready;
                }
                thumbnail_path = Some(path);
            } else if media.thumbnail_failed(&event_id.0) {
                dto.media_state = MediaState::Failed;
            }
        }
    }

    let avatar_path = media.avatar_path(&m.sender);
    if let Some(path) = &avatar_path
        && let Some(img) = peek_avatar(path)
    {
        dto.avatar = Some(img);
        dto.has_avatar = true;
    }

    record_media_need(&m.unique_id, thumbnail_path, avatar_path);
    dto
}

pub fn enrich_to_update(delta: &EnrichmentDelta, media: &dyn MediaCache) -> EnrichUpdate {
    let thumbnail = match delta.thumbnail {
        ThumbnailOutcome::Ready => delta
            .event_id
            .as_ref()
            .and_then(|event_id| media.thumbnail_path(&event_id.0))
            .and_then(|thumb_path| load_thumbnail(&thumb_path, &delta.unique_id))
            .map_or(ThumbUpdate::Unchanged, ThumbUpdate::Ready),
        ThumbnailOutcome::Failed => ThumbUpdate::Failed,
        ThumbnailOutcome::Unchanged => ThumbUpdate::Unchanged,
    };

    let avatar = if delta.avatar_ready {
        media.avatar_path(&delta.sender).and_then(|avatar_path| {
            load_avatar_async(&avatar_path, AvatarSlot::Message(delta.unique_id.clone()))
        })
    } else {
        None
    };

    let pronouns = delta.pronouns.as_ref().map(|pronouns| {
        pronoun_labels(pronouns)
            .into_iter()
            .map(SharedString::from)
            .collect()
    });

    EnrichUpdate {
        thumbnail,
        avatar,
        pronouns,
    }
}

pub fn room_to_dto(r: &Room, media: &dyn MediaCache) -> RoomDto {
    let mut dto = RoomDto {
        id: SharedString::from(r.id.as_ref()),
        name: SharedString::from(&r.display_name),
        initial: SharedString::from(avatar_initials(&r.display_name)),
        color_index: avatar_color_index(r.id.as_ref()),
        members: if r.is_direct {
            0
        } else {
            count(r.member_count)
        },
        unread: count(r.unread_count),
        mentions: count(r.mention_count),
        last_message_sender: SharedString::from(
            r.last_message_sender.as_deref().unwrap_or_default(),
        ),
        last_message_kind: preview_kind(r.last_message_kind),
        last_message_body: SharedString::from(&r.last_message_body),
        last_message_service_kind: r
            .last_message_service
            .as_ref()
            .map_or(ServiceKind::None, service_kind),
        last_message_service_target: SharedString::from(
            r.last_message_service.as_ref().map_or("", service_target),
        ),
        last_message_is_own: r.last_message_is_own,
        last_message_edited: r.last_message_edited,
        last_message_time: SharedString::from(&room_activity_label(r.last_activity_ts)),
        avatar: None,
        has_avatar: false,
    };

    if let Some(mxc) = &r.avatar_mxc
        && let Some(avatar_path) = media.room_avatar_path(mxc)
    {
        if let Some(img) = peek_avatar(&avatar_path) {
            dto.avatar = Some(img);
            dto.has_avatar = true;
        }
        record_avatar_need(AvatarSlot::Room(r.id.as_ref().to_owned()), avatar_path);
    }

    dto
}

pub fn space_to_dto(s: &Space, media: &dyn MediaCache) -> SpaceDto {
    let mut dto = SpaceDto {
        id: SharedString::from(&s.id),
        name: SharedString::from(&s.name),
        unread: count(s.unread),
        mentions: count(s.mentions),
        initial: SharedString::from(sender_initial(&s.name)),
        avatar: None,
        has_avatar: false,
    };

    if let Some(mxc) = &s.avatar_mxc
        && let Some(avatar_path) = media.space_avatar_path(mxc)
        && let Some(img) = load_avatar_async(&avatar_path, AvatarSlot::Space(s.id.clone()))
    {
        dto.avatar = Some(img);
        dto.has_avatar = true;
    }

    dto
}
