use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

use super::reactions;
use crate::domain::auth::Session;
use crate::domain::media::{AudioKind, AudioMeta, ImageMeta, VideoMeta, Waveform};
use crate::domain::message::{
    MessageBody, MessagePreviewKind, Reaction, ReactionSend, ReplyInfo, RichText, SendState,
    ServiceEvent, TimelineMessage,
};
use crate::domain::room::{NotifyMode, Room, RoomId, Space};
use crate::domain::sticker::{PackId, StickerImage, StickerPack};

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DemoData {
    pub session: SessionDto,
    #[serde(default)]
    pub rooms: Vec<RoomDto>,
    #[serde(default)]
    pub spaces: Vec<SpaceDto>,
    #[serde(default)]
    pub timelines: HashMap<String, Vec<MessageDto>>,
    #[serde(default)]
    pub pronouns: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub sticker_packs: Vec<StickerPackDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StickerPackDto {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub rooms: Vec<String>,
    #[serde(default)]
    pub images: Vec<PackImageDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackImageDto {
    pub shortcode: String,
    #[serde(default)]
    pub body: Option<String>,
    pub asset: String,
}

impl StickerPackDto {
    pub fn covers(&self, room_id: &str) -> bool {
        self.rooms.is_empty() || self.rooms.iter().any(|id| id == room_id)
    }

    pub fn to_pack(&self) -> StickerPack {
        StickerPack {
            id: PackId::new(self.id.clone()),
            title: self.title.clone(),
            images: self.images.iter().map(PackImageDto::to_image).collect(),
        }
    }
}

impl PackImageDto {
    fn to_image(&self) -> StickerImage {
        StickerImage {
            shortcode: self.shortcode.clone(),
            body: self.body.clone().unwrap_or_else(|| self.shortcode.clone()),
            mxc: format!("mxc://demo.local/{}", self.asset),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SessionDto {
    pub user_id: String,
    pub device_id: String,
    pub homeserver: String,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum NotifyDto {
    #[default]
    All,
    Mentions,
    Muted,
}

impl NotifyDto {
    fn to_mode(self) -> NotifyMode {
        match self {
            Self::All => NotifyMode::AllMessages,
            Self::Mentions => NotifyMode::MentionsOnly,
            Self::Muted => NotifyMode::Muted,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomDto {
    pub id: String,
    name: String,
    avatar: Option<String>,
    #[serde(default)]
    direct: bool,
    #[serde(default)]
    members: u64,
    #[serde(default)]
    pub unread: u64,
    #[serde(default)]
    mentions: u64,
    #[serde(default)]
    notify: NotifyDto,
    #[serde(default)]
    minutes_ago: u64,
    #[serde(default)]
    days_ago: u64,
    #[serde(default)]
    pub last_message: LastMessageDto,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct LastMessageDto {
    pub sender: Option<String>,
    pub sender_id: Option<String>,
    #[serde(default)]
    kind: KindDto,
    #[serde(default)]
    pub body: String,
    service: Option<ServiceDto>,
    #[serde(default)]
    pub own: bool,
    #[serde(default)]
    edited: bool,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum KindDto {
    #[default]
    Text,
    Image,
    Video,
    Audio,
    Voice,
    File,
    Location,
    Encrypted,
    Sticker,
    None,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpaceDto {
    id: String,
    name: String,
    avatar: Option<String>,
    #[serde(default)]
    rooms: Vec<String>,
    #[serde(default)]
    spaces: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageDto {
    id: String,
    sender: String,
    name: String,
    #[serde(default)]
    body: String,
    html: Option<String>,
    #[serde(default)]
    minutes_ago: u64,
    #[serde(default)]
    days_ago: u64,
    #[serde(default)]
    edited: bool,
    image: Option<ImageDto>,
    sticker: Option<StickerDto>,
    video: Option<VideoDto>,
    audio: Option<AudioDto>,
    reply: Option<ReplyDto>,
    service: Option<ServiceDto>,
    #[serde(default)]
    reactions: Vec<ReactionDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReactionDto {
    key: String,
    #[serde(default)]
    senders: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageDto {
    width: u32,
    height: u32,
    mimetype: Option<String>,
    filename: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VideoDto {
    width: u32,
    height: u32,
    #[serde(default)]
    duration_secs: u64,
    mimetype: Option<String>,
    filename: Option<String>,
    size: Option<u64>,
}

impl VideoDto {
    fn to_meta(&self) -> VideoMeta {
        VideoMeta {
            image: ImageMeta {
                width: Some(self.width),
                height: Some(self.height),
                mimetype: Some(
                    self.mimetype
                        .clone()
                        .unwrap_or_else(|| "video/mp4".to_owned()),
                ),
                filename: self.filename.clone(),
            },
            duration: (self.duration_secs > 0).then(|| Duration::from_secs(self.duration_secs)),
            size: self.size,
        }
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum AudioKindDto {
    Voice,
    Track,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioDto {
    kind: AudioKindDto,
    #[serde(default)]
    duration_secs: u64,
    mimetype: Option<String>,
    filename: Option<String>,
    size: Option<u64>,
    #[serde(default)]
    waveform: Vec<u16>,
}

impl AudioDto {
    fn to_meta(&self) -> AudioMeta {
        let (kind, filename, mimetype) = match self.kind {
            AudioKindDto::Voice => (AudioKind::Voice, "voice-message.ogg", "audio/ogg"),
            AudioKindDto::Track => (AudioKind::Track, "audio.m4a", "audio/mp4"),
        };
        AudioMeta {
            kind,
            filename: self.filename.clone().unwrap_or_else(|| filename.to_owned()),
            mimetype: Some(self.mimetype.clone().unwrap_or_else(|| mimetype.to_owned())),
            duration: (self.duration_secs > 0).then(|| Duration::from_secs(self.duration_secs)),
            size: self.size,
            waveform: Waveform::from_amplitudes(self.waveform.iter().copied()),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StickerDto {
    width: u32,
    height: u32,
    #[serde(default)]
    animated: bool,
}

impl StickerDto {
    fn to_meta(&self) -> ImageMeta {
        let mimetype = if self.animated {
            "image/webp"
        } else {
            "image/png"
        };
        ImageMeta {
            width: Some(self.width),
            height: Some(self.height),
            mimetype: Some(mimetype.to_owned()),
            filename: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum ServiceDto {
    Joined,
    Left,
    Invited {
        #[serde(default)]
        target: String,
    },
    Kicked {
        #[serde(default)]
        target: String,
    },
    Banned {
        #[serde(default)]
        target: String,
    },
    NameChanged {
        #[serde(default)]
        target: String,
    },
    AvatarChanged,
    RoomName {
        #[serde(default)]
        target: String,
    },
    RoomAvatar,
    RoomCreated,
    Encryption,
    CallStarted,
}

impl ServiceDto {
    fn to_event(&self) -> ServiceEvent {
        match self {
            Self::Joined => ServiceEvent::Joined,
            Self::Left => ServiceEvent::Left,
            Self::Invited { target } => ServiceEvent::Invited {
                target: optional(target),
            },
            Self::Kicked { target } => ServiceEvent::Kicked {
                target: optional(target),
            },
            Self::Banned { target } => ServiceEvent::Banned {
                target: optional(target),
            },
            Self::NameChanged { target } => ServiceEvent::DisplayNameChanged {
                name: target.clone(),
            },
            Self::AvatarChanged => ServiceEvent::AvatarChanged,
            Self::RoomName { target } => ServiceEvent::RoomNameChanged {
                name: target.clone(),
            },
            Self::RoomAvatar => ServiceEvent::RoomAvatarChanged,
            Self::RoomCreated => ServiceEvent::RoomCreated,
            Self::Encryption => ServiceEvent::EncryptionEnabled,
            Self::CallStarted => ServiceEvent::CallStarted,
        }
    }
}

fn optional(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyDto {
    sender: String,
    #[serde(default)]
    event_id: String,
    #[serde(default)]
    kind: KindDto,
    #[serde(default)]
    body: String,
}

impl SessionDto {
    pub fn to_session(&self) -> Session {
        Session {
            user_id: self.user_id.clone(),
            device_id: self.device_id.clone(),
            homeserver: self.homeserver.clone(),
            access_token: "demo-access-token".to_owned(),
            refresh_token: None,
            client_id: None,
        }
    }
}

impl RoomDto {
    pub fn to_room(&self, now_ms: u64) -> Room {
        Room {
            id: RoomId::new(&self.id),
            display_name: self.name.clone(),
            avatar_mxc: self.avatar.clone(),
            is_direct: self.direct,
            member_count: self.members,
            has_unread: self.unread > 0 && matches!(self.notify, NotifyDto::All),
            has_mentions: self.mentions > 0,
            has_activity: self.unread > 0,
            notify: self.notify.to_mode(),
            last_activity_ts: ago_ms(now_ms, self.minutes_ago, self.days_ago),
            last_message_sender: self.last_message.sender.clone(),
            last_message_kind: self.last_message.kind.to_kind(),
            last_message_body: self.last_message.body.clone(),
            last_message_service: self.last_message.service.as_ref().map(ServiceDto::to_event),
            last_message_is_own: self.last_message.own,
            last_message_edited: self.last_message.edited,
        }
    }
}

impl SpaceDto {
    pub fn to_space(&self) -> Space {
        Space {
            id: self.id.clone(),
            name: self.name.clone(),
            avatar_mxc: self.avatar.clone(),
            child_room_ids: self.rooms.clone(),
            child_space_ids: self.spaces.clone(),
            order: None,
            alert: false,
            mention: false,
            hint: false,
        }
    }
}

impl MessageDto {
    pub fn to_message(&self, own_user: &str, now_ms: u64) -> TimelineMessage {
        TimelineMessage {
            unique_id: self.id.clone(),
            event_id: Some(self.id.clone()),
            local_id: None,
            sender_pronouns: super::data::pronouns(&self.sender),
            sender: self.sender.clone(),
            sender_display_name: Some(self.name.clone()),
            sender_avatar_url: Some(self.sender.clone()),
            body: self.to_body(),
            timestamp: ago_ms(now_ms, self.minutes_ago, self.days_ago),
            is_own: self.sender == own_user,
            reply: self.reply.as_ref().map(|reply| ReplyInfo {
                event_id: reply.event_id.clone(),
                sender: reply.sender.clone(),
                kind: reply.kind.to_kind(),
                body: reply.body.clone(),
            }),
            edited: self.edited,
            is_first_unread: false,
            send_state: SendState::default(),
            reactions: self
                .reactions
                .iter()
                .map(|reaction| Reaction {
                    key: reaction.key.clone(),
                    mine: reaction.senders.iter().any(|sender| sender == own_user),
                    senders: reaction
                        .senders
                        .iter()
                        .map(|sender| reactions::reactor(sender))
                        .collect(),
                    send: ReactionSend::default(),
                })
                .collect(),
        }
    }

    fn to_body(&self) -> MessageBody {
        if let Some(service) = &self.service {
            return MessageBody::Service(service.to_event());
        }
        if let Some(sticker) = &self.sticker {
            return MessageBody::Sticker {
                alt: self.body.clone(),
                meta: sticker.to_meta(),
            };
        }
        if let Some(audio) = &self.audio {
            return MessageBody::Audio {
                caption: (!self.body.is_empty()).then(|| self.rich_body()),
                meta: audio.to_meta(),
            };
        }
        if let Some(video) = &self.video {
            return MessageBody::Video {
                caption: (!self.body.is_empty()).then(|| self.rich_body()),
                meta: video.to_meta(),
            };
        }
        match &self.image {
            Some(image) => MessageBody::Image {
                caption: (!self.body.is_empty()).then(|| self.rich_body()),
                meta: ImageMeta {
                    width: Some(image.width),
                    height: Some(image.height),
                    mimetype: Some(
                        image
                            .mimetype
                            .clone()
                            .unwrap_or_else(|| "image/png".to_owned()),
                    ),
                    filename: image.filename.clone(),
                },
            },
            None => MessageBody::Text(self.rich_body()),
        }
    }

    fn rich_body(&self) -> RichText {
        match &self.html {
            Some(html) => RichText::formatted(self.body.clone(), html.clone()),
            None => RichText::plain(self.body.clone()),
        }
    }
}

impl KindDto {
    fn to_kind(self) -> MessagePreviewKind {
        match self {
            Self::Text => MessagePreviewKind::Text,
            Self::Image => MessagePreviewKind::Image,
            Self::Video => MessagePreviewKind::Video,
            Self::Audio => MessagePreviewKind::Audio,
            Self::Voice => MessagePreviewKind::Voice,
            Self::File => MessagePreviewKind::File,
            Self::Location => MessagePreviewKind::Location,
            Self::Encrypted => MessagePreviewKind::Encrypted,
            Self::Sticker => MessagePreviewKind::Sticker,
            Self::None => MessagePreviewKind::None,
        }
    }
}

fn ago_ms(now_ms: u64, minutes: u64, days: u64) -> u64 {
    let minutes = minutes.saturating_add(days.saturating_mul(24 * 60));
    now_ms.saturating_sub(minutes.saturating_mul(60_000))
}
