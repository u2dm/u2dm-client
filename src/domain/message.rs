use std::hash::{DefaultHasher, Hash, Hasher};

use crate::domain::media::{
    AudioKind, AudioMeta, ContentKey, FileMeta, ImageMeta, MediaKind, VideoMeta,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessagePreviewKind {
    #[default]
    None,
    Text,
    Image,
    Video,
    Audio,
    Voice,
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RichText {
    pub plain: String,
    pub html: Option<String>,
}

impl RichText {
    pub fn plain(plain: String) -> Self {
        Self { plain, html: None }
    }

    pub fn formatted(plain: String, html: String) -> Self {
        Self {
            plain,
            html: Some(html),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageBody {
    Text(RichText),
    Notice(RichText),
    Emote(RichText),
    Image {
        caption: Option<RichText>,
        meta: ImageMeta,
    },
    Sticker {
        alt: String,
        meta: ImageMeta,
    },
    Video {
        caption: Option<RichText>,
        meta: VideoMeta,
    },
    Audio {
        caption: Option<RichText>,
        meta: AudioMeta,
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
            Self::Video { .. } => MessagePreviewKind::Video,
            Self::Sticker { .. } => MessagePreviewKind::Sticker,
            Self::Audio { meta, .. } => match meta.kind {
                AudioKind::Voice => MessagePreviewKind::Voice,
                AudioKind::Track => MessagePreviewKind::Audio,
            },
            Self::File { .. } => MessagePreviewKind::File,
            Self::UnableToDecrypt => MessagePreviewKind::Encrypted,
        }
    }

    pub fn audio(&self) -> Option<&AudioMeta> {
        match self {
            Self::Audio { meta, .. } => Some(meta),
            _ => None,
        }
    }

    pub fn media(&self) -> Option<(MediaKind, &ImageMeta)> {
        match self {
            Self::Image { meta, .. } => Some((MediaKind::Photo, meta)),
            Self::Sticker { meta, .. } => Some((MediaKind::Sticker, meta)),
            Self::Video { meta, .. } => Some((MediaKind::Video, &meta.image)),
            Self::Text(_)
            | Self::Notice(_)
            | Self::Emote(_)
            | Self::Audio { .. }
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

pub const REACTOR_AVATAR_LIMIT: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reactor {
    pub user_id: String,
    pub avatar_url: Option<String>,
}

impl Reactor {
    pub fn new(user_id: String) -> Self {
        Self {
            user_id,
            avatar_url: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    pub key: String,
    pub senders: Vec<Reactor>,
    pub mine: bool,
    pub send: ReactionSend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReactionSend {
    #[default]
    Sent,
    Sending,
    Failed,
}

impl Reaction {
    pub fn count(&self) -> usize {
        self.senders.len()
    }

    pub fn shows_reactors(&self) -> bool {
        self.senders.len() < REACTOR_AVATAR_LIMIT
    }
}

const PROGRESS_SCALE: u16 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SendState {
    #[default]
    Sent,
    Sending,
    Uploading {
        sent: u64,
        total: u64,
    },
    Failed,
}

impl SendState {
    pub fn fraction(self) -> f32 {
        match self {
            Self::Uploading { sent, total } if total > 0 => {
                let scaled = sent.min(total).saturating_mul(u64::from(PROGRESS_SCALE)) / total;
                f32::from(u16::try_from(scaled).unwrap_or(PROGRESS_SCALE))
                    / f32::from(PROGRESS_SCALE)
            }
            _ => 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineMessage {
    pub unique_id: String,
    pub event_id: Option<String>,
    pub local_id: Option<String>,
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
    pub send_state: SendState,
    pub reactions: Vec<Reaction>,
}

impl TimelineMessage {
    pub fn thumbnail_content(&self) -> Option<&ContentKey> {
        self.body.media()?.1.thumbnail.as_ref()
    }

    pub fn enrichment_fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.event_id.hash(&mut hasher);
        self.local_id.hash(&mut hasher);
        self.sender.hash(&mut hasher);
        self.sender_avatar_url.hash(&mut hasher);
        match self.body.media() {
            Some((kind, meta)) => {
                kind.hash(&mut hasher);
                meta.width.hash(&mut hasher);
                meta.height.hash(&mut hasher);
                meta.mimetype.hash(&mut hasher);
                meta.thumbnail.hash(&mut hasher);
            }
            None => None::<MediaKind>.hash(&mut hasher),
        }
        hasher.finish()
    }
}
