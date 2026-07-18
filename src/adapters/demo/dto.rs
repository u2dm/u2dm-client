use std::collections::HashMap;

use serde::Deserialize;

use crate::domain::models::{
    EventId, ImageMeta, MessageBody, MessagePreviewKind, ReplyInfo, Room, RoomId, ServiceEvent,
    Session, Space, TimelineMessage,
};

#[derive(Deserialize, Default)]
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
}

#[derive(Deserialize, Default)]
pub struct SessionDto {
    pub user_id: String,
    pub device_id: String,
    pub homeserver: String,
}

#[derive(Deserialize)]
pub struct RoomDto {
    pub id: String,
    name: String,
    avatar: Option<String>,
    #[serde(default)]
    direct: bool,
    #[serde(default)]
    members: u64,
    #[serde(default)]
    unread: u64,
    #[serde(default)]
    mentions: u64,
    #[serde(default)]
    minutes_ago: u64,
    #[serde(default)]
    days_ago: u64,
    #[serde(default)]
    pub last_message: LastMessageDto,
}

#[derive(Deserialize, Default)]
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
    File,
    Location,
    Encrypted,
    Sticker,
    None,
}

#[derive(Deserialize)]
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
pub struct MessageDto {
    id: String,
    sender: String,
    name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    minutes_ago: u64,
    #[serde(default)]
    days_ago: u64,
    #[serde(default)]
    edited: bool,
    image: Option<ImageDto>,
    reply: Option<ReplyDto>,
    service: Option<ServiceDto>,
}

#[derive(Deserialize)]
struct ImageDto {
    width: u32,
    height: u32,
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
struct ReplyDto {
    sender: String,
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
            unread_count: self.unread,
            mention_count: self.mentions,
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
            unread: 0,
            mentions: 0,
        }
    }
}

impl MessageDto {
    pub fn to_message(&self, own_user: &str, now_ms: u64) -> TimelineMessage {
        TimelineMessage {
            unique_id: self.id.clone(),
            event_id: Some(EventId(self.id.clone())),
            sender_pronouns: super::data::pronouns(&self.sender),
            sender: self.sender.clone(),
            sender_display_name: Some(self.name.clone()),
            sender_avatar_url: None,
            body: self.to_body(),
            timestamp: ago_ms(now_ms, self.minutes_ago, self.days_ago),
            is_own: self.sender == own_user,
            reply: self.reply.as_ref().map(|reply| ReplyInfo {
                sender: reply.sender.clone(),
                kind: reply.kind.to_kind(),
                body: reply.body.clone(),
            }),
            edited: self.edited,
        }
    }

    fn to_body(&self) -> MessageBody {
        if let Some(service) = &self.service {
            return MessageBody::Service(service.to_event());
        }
        match &self.image {
            Some(image) => MessageBody::Image {
                caption: (!self.body.is_empty()).then(|| self.body.clone()),
                meta: ImageMeta {
                    width: Some(image.width),
                    height: Some(image.height),
                    mimetype: Some("image/png".to_owned()),
                },
            },
            None => MessageBody::Text(self.body.clone()),
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
