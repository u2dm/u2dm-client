use std::hash::{DefaultHasher, Hash, Hasher};

use crate::domain::media::{FileMeta, ImageMeta, MediaKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessagePreviewKind {
    #[default]
    None,
    Text,
    Image,
    Video,
    Audio,
    File,
    Location,
    Encrypted,
    Sticker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceEvent {
    Joined,
    Left,
    Invited { target: Option<String> },
    InvitationAccepted,
    InvitationRejected,
    InvitationRevoked { target: Option<String> },
    Kicked { target: Option<String> },
    Banned { target: Option<String> },
    Unbanned { target: Option<String> },
    Knocked,
    KnockAccepted { target: Option<String> },
    DisplayNameSet { name: String },
    DisplayNameChanged { name: String },
    DisplayNameRemoved,
    AvatarChanged,
    RoomNameChanged { name: String },
    RoomTopicChanged,
    RoomAvatarChanged,
    RoomCreated,
    EncryptionEnabled,
    CallStarted,
    CallNotification,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageBody {
    Text(String),
    Notice(String),
    Emote(String),
    Image {
        caption: Option<String>,
        meta: ImageMeta,
    },
    Sticker {
        alt: String,
        meta: ImageMeta,
    },
    File {
        meta: FileMeta,
    },
    Service(ServiceEvent),
    UnableToDecrypt,
    Unsupported {
        kind: String,
        fallback: String,
    },
}

impl MessageBody {
    pub fn service(&self) -> Option<&ServiceEvent> {
        match self {
            Self::Service(event) => Some(event),
            _ => None,
        }
    }

    pub fn preview_kind(&self) -> MessagePreviewKind {
        match self {
            Self::Text(_)
            | Self::Notice(_)
            | Self::Emote(_)
            | Self::Service(_)
            | Self::Unsupported { .. } => MessagePreviewKind::Text,
            Self::Image { .. } => MessagePreviewKind::Image,
            Self::Sticker { .. } => MessagePreviewKind::Sticker,
            Self::File { .. } => MessagePreviewKind::File,
            Self::UnableToDecrypt => MessagePreviewKind::Encrypted,
        }
    }

    pub fn media(&self) -> Option<(MediaKind, &ImageMeta)> {
        match self {
            Self::Image { meta, .. } => Some((MediaKind::Photo, meta)),
            Self::Sticker { meta, .. } => Some((MediaKind::Sticker, meta)),
            Self::Text(_)
            | Self::Notice(_)
            | Self::Emote(_)
            | Self::File { .. }
            | Self::Service(_)
            | Self::UnableToDecrypt
            | Self::Unsupported { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyInfo {
    pub event_id: String,
    pub sender: String,
    pub kind: MessagePreviewKind,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineMessage {
    pub unique_id: String,
    pub event_id: Option<String>,
    pub sender: String,
    pub sender_display_name: Option<String>,
    pub sender_avatar_url: Option<String>,
    pub sender_pronouns: Vec<String>,
    pub body: MessageBody,
    pub timestamp: u64,
    pub is_own: bool,
    pub reply: Option<ReplyInfo>,
    pub edited: bool,
    pub is_first_unread: bool,
}

impl TimelineMessage {
    pub fn enrichment_fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.event_id.hash(&mut hasher);
        self.sender.hash(&mut hasher);
        self.sender_avatar_url.hash(&mut hasher);
        match self.body.media() {
            Some((kind, meta)) => {
                kind.hash(&mut hasher);
                meta.width.hash(&mut hasher);
                meta.height.hash(&mut hasher);
                meta.mimetype.hash(&mut hasher);
            }
            None => None::<MediaKind>.hash(&mut hasher),
        }
        hasher.finish()
    }
}
