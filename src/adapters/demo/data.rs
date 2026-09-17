use std::fs;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use super::dto::{DemoData, RoomDto, SpaceDto, StickerPackDto};
use super::media;
use super::timeline::{self, scenario};
use crate::domain::auth::Session;
use crate::domain::media::{
    AudioKind, AudioMeta, FileMeta, ImageMeta, OutgoingAttachment, VideoMeta,
};
use crate::domain::message::{MessageBody, ReplyInfo, RichText, SendState, TimelineMessage};
use crate::domain::room::{Room, RoomId, Space};
use crate::domain::sticker::{StickerImage, StickerPack};

const UNKNOWN_SENDER: &str = "@member:matrix.org";
const SENT_STICKER_EXTENT: u32 = 512;
const STICKER_ASSET_MARKER: char = '#';

static DATA: OnceLock<DemoData> = OnceLock::new();
static LOAD_ERROR: OnceLock<String> = OnceLock::new();

fn data() -> &'static DemoData {
    DATA.get_or_init(load)
}

fn load() -> DemoData {
    let path = media::data_path();
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            tracing::error!("demo data {} could not be read: {e}", path.display());
            drop(LOAD_ERROR.set(format!("{} could not be read: {e}", path.display())));
            return DemoData::default();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(data) => data,
        Err(e) => {
            tracing::error!("demo data {} is not valid: {e}", path.display());
            drop(LOAD_ERROR.set(format!("{} is not valid: {e}", path.display())));
            DemoData::default()
        }
    }
}

pub fn load_error() -> Option<&'static str> {
    LOAD_ERROR.get().map(String::as_str)
}

pub fn source_path() -> String {
    media::data_path().display().to_string()
}

pub fn counts() -> (usize, usize, usize) {
    let data = data();
    (data.rooms.len(), data.spaces.len(), data.timelines.len())
}

pub fn own_user() -> &'static str {
    &data().session.user_id
}

pub fn session() -> Session {
    data().session.to_session()
}

pub fn rooms() -> Vec<Arc<Room>> {
    let now = now_ms();
    data()
        .rooms
        .iter()
        .map(|room| Arc::new(room.to_room(now)))
        .collect()
}

pub fn spaces() -> Vec<Space> {
    data().spaces.iter().map(SpaceDto::to_space).collect()
}

pub fn sticker_packs(room_id: &RoomId) -> Vec<StickerPack> {
    data()
        .sticker_packs
        .iter()
        .filter(|pack| pack.covers(room_id))
        .map(StickerPackDto::to_pack)
        .collect()
}

pub fn sticker_image(pack_id: &str, shortcode: &str) -> Option<StickerImage> {
    data()
        .sticker_packs
        .iter()
        .find(|pack| pack.id == pack_id)?
        .to_pack()
        .images
        .into_iter()
        .find(|image| image.shortcode == shortcode)
}

pub fn messages(room_id: &RoomId) -> Vec<TimelineMessage> {
    let now = now_ms();
    let mut messages = match data().timelines.get(room_id.as_ref()) {
        Some(timeline) => timeline
            .iter()
            .map(|message| message.to_message(own_user(), now))
            .collect(),
        None => last_message_only(room_id),
    };
    if scenario().history_is_long {
        messages = repeated_history(&messages);
    }
    super::richtext::apply_scenario(&mut messages);
    super::reactions::apply_scenario(&mut messages);
    mark_first_unread(room_id, &mut messages);
    messages
}

fn repeated_history(messages: &[TimelineMessage]) -> Vec<TimelineMessage> {
    let mut repeated = Vec::with_capacity(messages.len() * timeline::HISTORY_COPIES);
    for copy in 0..timeline::HISTORY_COPIES {
        for message in messages {
            let mut message = message.clone();
            message.unique_id = format!("{}-copy{copy}", message.unique_id);
            message.event_id = message
                .event_id
                .as_ref()
                .map(|id| format!("{id}-copy{copy}"));
            repeated.push(message);
        }
    }
    repeated
}

fn unread_count(room_id: &RoomId, loaded: usize) -> usize {
    if scenario().read_position_precedes_history {
        return loaded;
    }
    if scenario().history_is_long {
        return loaded * timeline::UNREAD_PORTION_NUMERATOR / timeline::UNREAD_PORTION_DENOMINATOR;
    }
    data()
        .rooms
        .iter()
        .find(|room| room.id == room_id.as_ref())
        .map_or(0, |room| usize::try_from(room.unread).unwrap_or(usize::MAX))
}

fn mark_first_unread(room_id: &RoomId, messages: &mut [TimelineMessage]) {
    let unread = unread_count(room_id, messages.len());
    if unread == 0 {
        return;
    }
    let read_up_to = messages.len().saturating_sub(unread);
    if let Some(message) = messages.iter_mut().skip(read_up_to).find(|m| !m.is_own) {
        message.is_first_unread = true;
    }
}

pub fn pronouns(user_id: &str) -> Vec<String> {
    data().pronouns.get(user_id).cloned().unwrap_or_default()
}

pub fn own_message(sequence: u64, body: &str, reply: Option<ReplyInfo>) -> TimelineMessage {
    let id = format!("demo-sent-{sequence}");
    TimelineMessage {
        unique_id: id.clone(),
        event_id: Some(id),
        local_id: None,
        sender_pronouns: Vec::new(),
        sender: own_user().to_owned(),
        sender_display_name: Some("You".to_owned()),
        sender_avatar_url: Some(own_user().to_owned()),
        body: MessageBody::Text(RichText::plain(body.to_owned())),
        timestamp: now_ms(),
        is_own: true,
        reply,
        edited: false,
        is_first_unread: false,
        send_state: SendState::default(),
        reactions: Vec::new(),
    }
}

pub fn own_sticker(
    sequence: u64,
    image: &StickerImage,
    reply: Option<ReplyInfo>,
) -> TimelineMessage {
    let id = format!(
        "demo-sent-{sequence}{STICKER_ASSET_MARKER}{}",
        media::mxc_asset(&image.mxc)
    );
    TimelineMessage {
        unique_id: id.clone(),
        event_id: Some(id),
        local_id: None,
        sender_pronouns: Vec::new(),
        sender: own_user().to_owned(),
        sender_display_name: Some("You".to_owned()),
        sender_avatar_url: Some(own_user().to_owned()),
        body: MessageBody::Sticker {
            alt: image.body.clone(),
            meta: ImageMeta {
                width: Some(SENT_STICKER_EXTENT),
                height: Some(SENT_STICKER_EXTENT),
                mimetype: None,
                filename: None,
            },
        },
        timestamp: now_ms(),
        is_own: true,
        reply,
        edited: false,
        is_first_unread: false,
        send_state: SendState::default(),
        reactions: Vec::new(),
    }
}

pub fn own_attachment(
    sequence: u64,
    attachment: &OutgoingAttachment,
    reply: Option<ReplyInfo>,
) -> TimelineMessage {
    let id = format!("demo-sent-{sequence}");
    let picked = &attachment.picked;
    let caption = || {
        attachment
            .caption
            .as_ref()
            .map(|text| RichText::plain(text.clone()))
    };
    let (width, height) = picked.dimensions.unzip();
    let image_meta = || ImageMeta {
        width,
        height,
        mimetype: Some(picked.mimetype.clone()),
        filename: Some(picked.filename.clone()),
    };
    let body = if attachment.as_document {
        MessageBody::File {
            meta: FileMeta {
                filename: picked.filename.clone(),
                mimetype: Some(picked.mimetype.clone()),
                size: Some(picked.size),
            },
        }
    } else if picked.is_video() {
        MessageBody::Video {
            caption: caption(),
            meta: VideoMeta {
                image: image_meta(),
                duration: picked.duration,
                size: Some(picked.size),
            },
        }
    } else if picked.is_audio() {
        MessageBody::Audio {
            caption: caption(),
            meta: AudioMeta {
                kind: AudioKind::Track,
                filename: picked.filename.clone(),
                mimetype: Some(picked.mimetype.clone()),
                duration: picked.duration,
                size: Some(picked.size),
                waveform: picked.waveform.clone(),
            },
        }
    } else if picked.is_image() {
        MessageBody::Image {
            caption: caption(),
            meta: image_meta(),
        }
    } else {
        MessageBody::File {
            meta: FileMeta {
                filename: picked.filename.clone(),
                mimetype: Some(picked.mimetype.clone()),
                size: Some(picked.size),
            },
        }
    };
    TimelineMessage {
        unique_id: id.clone(),
        event_id: Some(id),
        local_id: None,
        sender_pronouns: Vec::new(),
        sender: own_user().to_owned(),
        sender_display_name: Some("You".to_owned()),
        sender_avatar_url: Some(own_user().to_owned()),
        body,
        timestamp: now_ms(),
        is_own: true,
        reply,
        edited: false,
        is_first_unread: false,
        send_state: SendState::default(),
        reactions: Vec::new(),
    }
}

pub fn sticker_asset_in(event_id: &str) -> Option<&str> {
    event_id
        .split_once(STICKER_ASSET_MARKER)
        .map(|(_, asset)| asset)
}

pub fn body_preview(body: &MessageBody) -> String {
    match body {
        MessageBody::Text(text) | MessageBody::Notice(text) | MessageBody::Emote(text) => {
            text.plain.clone()
        }
        MessageBody::Image { caption, .. }
        | MessageBody::Video { caption, .. }
        | MessageBody::Audio { caption, .. } => caption
            .as_ref()
            .map(|text| text.plain.clone())
            .unwrap_or_default(),
        MessageBody::Sticker { alt, .. } => alt.clone(),
        MessageBody::File { meta } => meta.filename.clone(),
        MessageBody::Service(_) | MessageBody::UnableToDecrypt => String::new(),
        MessageBody::Unsupported { fallback, .. } => fallback.clone(),
    }
}

pub fn sender_label(message: &TimelineMessage) -> String {
    message
        .sender_display_name
        .clone()
        .unwrap_or_else(|| message.sender.clone())
}

fn last_message_only(room_id: &RoomId) -> Vec<TimelineMessage> {
    let Some(dto) = data().rooms.iter().find(|room| room.id == room_id.as_ref()) else {
        return Vec::new();
    };
    if dto.last_message.body.is_empty() {
        return Vec::new();
    }

    vec![synthesized_message(dto, &dto.to_room(now_ms()))]
}

fn synthesized_message(dto: &RoomDto, room: &Room) -> TimelineMessage {
    let (sender, display_name) = if dto.last_message.own {
        (own_user().to_owned(), "You".to_owned())
    } else {
        (
            dto.last_message
                .sender_id
                .clone()
                .unwrap_or_else(|| UNKNOWN_SENDER.to_owned()),
            dto.last_message.sender.clone().unwrap_or_default(),
        )
    };
    let id = format!("demo-{}-last", dto.id.trim_start_matches('!'));

    TimelineMessage {
        unique_id: id.clone(),
        event_id: Some(id),
        local_id: None,
        sender_pronouns: pronouns(&sender),
        sender_avatar_url: Some(sender.clone()),
        sender,
        sender_display_name: Some(display_name),
        body: MessageBody::Text(RichText::plain(room.last_message_body.clone())),
        timestamp: room.last_activity_ts,
        is_own: dto.last_message.own,
        reply: None,
        edited: room.last_message_edited,
        is_first_unread: false,
        send_state: SendState::default(),
        reactions: Vec::new(),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or_default()
}
