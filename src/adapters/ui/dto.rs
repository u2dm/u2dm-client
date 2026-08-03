use slint::{Image, SharedString};

use super::decode::{
    AvatarSlot, Decoded, load_avatar_async, load_thumbnail, peek_avatar, peek_thumbnail,
    record_avatar_need, record_media_need, record_sticker_need,
};
use super::present::{
    MessageKind, ServiceKind, avatar_color_index, avatar_initials, message_body_text, message_kind,
    message_sender_label, message_timestamp_label, pronoun_labels, room_activity_label,
    sender_initial, service_kind, service_target, unsupported_kind,
};
use super::schema::{define_ui_enum, media_states};
use crate::domain::models::{
    EnrichmentDelta, MessagePreviewKind, PackId, Room, Space, StickerImage, StickerPack,
    ThumbnailOutcome, TimelineMessage,
};
use crate::ports::media::MediaCache;

media_states!(define_ui_enum MediaState;);

pub const GRID_COLUMNS: i32 = 5;

#[derive(Clone)]
pub struct StickerCellDto {
    pub key: SharedString,
    pub pack_id: SharedString,
    pub shortcode: SharedString,
    pub label: SharedString,
    pub image: Option<Image>,
    pub media_state: MediaState,
}

pub struct StickerRowDto {
    pub title: SharedString,
    pub is_header: bool,
    pub cells: Vec<StickerCellDto>,
}

pub struct StickerPackDto {
    pub id: SharedString,
    pub title: SharedString,
    pub header_row: i32,
    pub icon: Option<Image>,
    pub icon_cell_key: SharedString,
}

pub struct StickerGrid {
    pub rows: Vec<StickerRowDto>,
    pub packs: Vec<StickerPackDto>,
}

pub const CELL_KEY_SEPARATOR: char = '\u{1}';

pub fn cell_key(pack: &PackId, shortcode: &str) -> String {
    format!("{pack}{CELL_KEY_SEPARATOR}{shortcode}")
}

pub enum DecodeTarget<'a> {
    Timeline { unique_id: &'a str },
    StickerCell { key: &'a str, pack: &'a str },
}

impl<'a> DecodeTarget<'a> {
    pub fn of(key: &'a str) -> Self {
        match key.split_once(CELL_KEY_SEPARATOR) {
            Some((pack, _)) => Self::StickerCell { key, pack },
            None => Self::Timeline { unique_id: key },
        }
    }
}

pub fn sticker_grid(packs: &[StickerPack], query: &str, media: &dyn MediaCache) -> StickerGrid {
    let per_row = usize::try_from(GRID_COLUMNS.max(1)).unwrap_or(1);
    let needle = query.trim().to_lowercase();
    let mut grid = StickerGrid {
        rows: Vec::new(),
        packs: Vec::new(),
    };

    for pack in packs {
        let whole_pack = needle.is_empty() || pack.title.to_lowercase().contains(&needle);
        let cells: Vec<StickerCellDto> = pack
            .images
            .iter()
            .filter(|image| whole_pack || sticker_matches(image, &needle))
            .map(|image| sticker_cell(pack, image, media))
            .collect();
        if cells.is_empty() {
            continue;
        }

        grid.packs.push(StickerPackDto {
            id: SharedString::from(pack.id.as_ref()),
            title: SharedString::from(&pack.title),
            header_row: i32::try_from(grid.rows.len()).unwrap_or(0),
            icon: cells.first().and_then(|cell| cell.image.clone()),
            icon_cell_key: cells
                .first()
                .map(|cell| cell.key.clone())
                .unwrap_or_default(),
        });
        grid.rows.push(StickerRowDto {
            title: SharedString::from(&pack.title),
            is_header: true,
            cells: Vec::new(),
        });
        for chunk in cells.chunks(per_row) {
            grid.rows.push(StickerRowDto {
                title: SharedString::new(),
                is_header: false,
                cells: chunk.to_vec(),
            });
        }
    }

    grid
}

fn sticker_matches(image: &StickerImage, needle: &str) -> bool {
    image.shortcode.to_lowercase().contains(needle) || image.body.to_lowercase().contains(needle)
}

fn sticker_cell(
    pack: &StickerPack,
    image: &StickerImage,
    media: &dyn MediaCache,
) -> StickerCellDto {
    let key = cell_key(&pack.id, &image.shortcode);
    let mut cell = StickerCellDto {
        key: SharedString::from(&key),
        pack_id: SharedString::from(pack.id.as_ref()),
        shortcode: SharedString::from(&image.shortcode),
        label: SharedString::from(&image.body),
        image: None,
        media_state: MediaState::Idle,
    };

    if let Some(path) = media.sticker_path(&image.mxc) {
        match peek_thumbnail(&path) {
            Decoded::Ready(decoded) => {
                cell.image = Some(decoded);
                cell.media_state = MediaState::Ready;
            }
            Decoded::Failed => cell.media_state = MediaState::Failed,
            Decoded::Pending => record_sticker_need(&key, path),
        }
    } else if media.sticker_failed(&image.mxc) {
        cell.media_state = MediaState::Failed;
    }

    cell
}

#[allow(clippy::struct_excessive_bools)]
pub struct MessageDto {
    pub unique_id: SharedString,
    pub sender: SharedString,
    pub pronouns: Vec<SharedString>,
    pub body: SharedString,
    pub timestamp: SharedString,
    pub message_type: MessageKind,
    pub preview_kind: MessagePreviewKind,
    pub unsupported_kind: SharedString,
    pub event_id: SharedString,
    pub sender_initial: SharedString,
    pub color_index: i32,
    pub is_own: bool,
    pub edited: bool,
    pub is_first_unread: bool,
    pub has_reply: bool,
    pub reply_event_id: SharedString,
    pub reply_sender: SharedString,
    pub reply_kind: MessagePreviewKind,
    pub reply_body: SharedString,
    pub service_kind: ServiceKind,
    pub service_target: SharedString,
    pub image_width: i32,
    pub image_height: i32,
    pub thumbnail: Option<Image>,
    pub media_state: MediaState,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
    pub needs_media: bool,
}

#[allow(clippy::struct_excessive_bools)]
pub struct RoomDto {
    pub id: SharedString,
    pub name: SharedString,
    pub initial: SharedString,
    pub color_index: i32,
    pub members: i32,
    pub alert: bool,
    pub mention: bool,
    pub hint: bool,
    pub muted: bool,
    pub last_message_sender: SharedString,
    pub last_message_kind: MessagePreviewKind,
    pub last_message_body: SharedString,
    pub last_message_service_kind: ServiceKind,
    pub last_message_service_target: SharedString,
    pub last_message_is_own: bool,
    pub last_message_edited: bool,
    pub last_message_time: SharedString,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
}

#[allow(clippy::struct_excessive_bools)]
pub struct SpaceDto {
    pub id: SharedString,
    pub name: SharedString,
    pub alert: bool,
    pub mention: bool,
    pub hint: bool,
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
        preview_kind: m.body.preview_kind(),
        unsupported_kind: SharedString::from(unsupported_kind(&m.body)),
        event_id: SharedString::from(m.event_id.as_deref().unwrap_or_default()),
        sender_initial: SharedString::from(avatar_initials(sender_label)),
        color_index: avatar_color_index(&m.sender),
        is_own: m.is_own,
        edited: m.edited,
        is_first_unread: m.is_first_unread,
        has_reply: m.reply.is_some(),
        reply_event_id: SharedString::from(m.reply.as_ref().map_or("", |r| r.event_id.as_str())),
        reply_sender: SharedString::from(m.reply.as_ref().map_or("", |r| r.sender.as_str())),
        reply_kind: m
            .reply
            .as_ref()
            .map_or(MessagePreviewKind::None, |r| r.kind),
        reply_body: SharedString::from(m.reply.as_ref().map_or("", |r| r.body.as_str())),
        service_kind: m.body.service().map_or(ServiceKind::None, service_kind),
        service_target: SharedString::from(m.body.service().map_or("", service_target)),
        image_width: 0,
        image_height: 0,
        thumbnail: None,
        media_state: MediaState::Idle,
        avatar: None,
        has_avatar: false,
        needs_media: false,
    };

    let mut thumbnail_path = None;
    if let Some((_, meta)) = m.body.media() {
        dto.image_width = meta.width.unwrap_or(0).cast_signed();
        dto.image_height = meta.height.unwrap_or(0).cast_signed();
        if let Some(event_id) = m.event_id.as_deref() {
            if let Some(path) = media.thumbnail_path(event_id) {
                match peek_thumbnail(&path) {
                    Decoded::Ready(img) => {
                        dto.thumbnail = Some(img);
                        dto.media_state = MediaState::Ready;
                    }
                    Decoded::Failed => dto.media_state = MediaState::Failed,
                    Decoded::Pending => {}
                }
                thumbnail_path = Some(path);
            } else if media.thumbnail_failed(event_id) {
                dto.media_state = MediaState::Failed;
            }
        }
    }

    let avatar_path = m
        .sender_avatar_url
        .as_deref()
        .and_then(|mxc| media.user_avatar_path(mxc));
    if let Some(path) = &avatar_path
        && let Some(img) = peek_avatar(path)
    {
        dto.avatar = Some(img);
        dto.has_avatar = true;
    }

    let thumbnail_undecoded = thumbnail_path.is_some() && dto.media_state == MediaState::Idle;
    let avatar_undecoded = avatar_path.is_some() && !dto.has_avatar;
    dto.needs_media = thumbnail_undecoded || avatar_undecoded;
    record_media_need(&m.unique_id, thumbnail_path, avatar_path);
    dto
}

pub fn enrich_to_update(delta: &EnrichmentDelta, media: &dyn MediaCache) -> EnrichUpdate {
    let thumbnail = match delta.thumbnail {
        ThumbnailOutcome::Ready => delta
            .event_id
            .as_deref()
            .and_then(|event_id| media.thumbnail_path(event_id))
            .map_or(ThumbUpdate::Unchanged, |thumb_path| {
                match load_thumbnail(&thumb_path, &delta.unique_id) {
                    Decoded::Ready(image) => ThumbUpdate::Ready(image),
                    Decoded::Failed => ThumbUpdate::Failed,
                    Decoded::Pending => ThumbUpdate::Unchanged,
                }
            }),
        ThumbnailOutcome::Failed => ThumbUpdate::Failed,
        ThumbnailOutcome::Unchanged => ThumbUpdate::Unchanged,
    };

    let avatar = delta
        .avatar_mxc
        .as_deref()
        .and_then(|mxc| media.user_avatar_path(mxc))
        .and_then(|avatar_path| {
            load_avatar_async(&avatar_path, AvatarSlot::Message(delta.unique_id.clone()))
        });

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
        alert: r.alert(),
        mention: r.mention(),
        hint: r.hint(),
        muted: r.muted(),
        last_message_sender: SharedString::from(
            r.last_message_sender.as_deref().unwrap_or_default(),
        ),
        last_message_kind: r.last_message_kind,
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
        && let Some(img) = peek_avatar(&avatar_path)
    {
        dto.avatar = Some(img);
        dto.has_avatar = true;
    }

    dto
}

pub fn record_room_avatar_need(r: &Room, media: &dyn MediaCache) {
    if let Some(mxc) = &r.avatar_mxc
        && let Some(avatar_path) = media.room_avatar_path(mxc)
    {
        record_avatar_need(AvatarSlot::Room(r.id.as_ref().to_owned()), avatar_path);
    }
}

pub fn space_to_dto(s: &Space, media: &dyn MediaCache) -> SpaceDto {
    let mut dto = SpaceDto {
        id: SharedString::from(&s.id),
        name: SharedString::from(&s.name),
        alert: s.alert,
        mention: s.mention,
        hint: s.hint,
        initial: SharedString::from(sender_initial(&s.name)),
        avatar: None,
        has_avatar: false,
    };

    if let Some(mxc) = &s.avatar_mxc
        && let Some(avatar_path) = media.space_avatar_path(mxc)
        && let Some(img) = peek_avatar(&avatar_path)
    {
        dto.avatar = Some(img);
        dto.has_avatar = true;
    }

    dto
}

pub fn prefetch_space_avatar(s: &Space, media: &dyn MediaCache) {
    if let Some(mxc) = &s.avatar_mxc
        && let Some(avatar_path) = media.space_avatar_path(mxc)
    {
        load_avatar_async(&avatar_path, AvatarSlot::Space(s.id.clone()));
    }
}
