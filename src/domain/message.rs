mod edits;
mod reactions;
mod receipts;
mod service;

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};

pub(crate) use self::edits::{EditKind, EditTarget, MessageEdit, TextRevision};
#[cfg(feature = "matrix")]
pub(crate) use self::reactions::REACTOR_AVATAR_LIMIT;
pub(crate) use self::reactions::{CustomEmoji, Reaction, ReactionSend, Reactor};
pub(crate) use self::receipts::{ReadBy, ReadScan};
pub(crate) use self::service::ServiceEvent;
use crate::domain::location::Location;
use crate::domain::media::{
    AudioKind, AudioMeta, ContentKey, FileMeta, ImageMeta, MediaKind, VideoMeta,
};
use crate::domain::member_name::{MemberName, NamedUser};
use crate::domain::poll::Poll;

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
    Poll,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Markup {
    #[default]
    Plain,
    Html(String),
    Markdown {
        html: String,
        composer: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RichText {
    pub plain: String,
    pub markup: Markup,
}

impl RichText {
    pub fn plain(plain: String) -> Self {
        Self {
            plain,
            markup: Markup::Plain,
        }
    }

    pub fn formatted(plain: String, html: String) -> Self {
        Self {
            plain,
            markup: Markup::Html(html),
        }
    }

    pub fn with_html(plain: String, html: Option<String>) -> Self {
        match html {
            Some(html) => Self::formatted(plain, html),
            None => Self::plain(plain),
        }
    }

    pub fn authored(plain: String, html: String, composer: String) -> Self {
        Self {
            plain,
            markup: Markup::Markdown { html, composer },
        }
    }

    pub fn html(&self) -> Option<&str> {
        match &self.markup {
            Markup::Plain => None,
            Markup::Html(html) | Markup::Markdown { html, .. } => Some(html),
        }
    }

    pub fn composer_text(&self) -> Option<&str> {
        match &self.markup {
            Markup::Plain => Some(&self.plain),
            Markup::Markdown { composer, .. } => Some(composer),
            Markup::Html(_) => None,
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
        caption: Option<RichText>,
        meta: FileMeta,
    },
    Location(Location),
    Poll(Poll),
    Service(ServiceEvent),
    UnableToDecrypt(DecryptionFailure),
    Unsupported {
        kind: String,
        fallback: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DecryptionFailure {
    #[default]
    Unexplained,
    SentBeforeJoining,
    WithheldFromUnverified,
    WithheldBySender,
    HistoryWithoutBackup,
    HistoryNeedsVerification,
    UnsignedDevice,
    UnknownDevice,
    VerificationViolation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShieldCode {
    AuthenticityNotGuaranteed,
    UnknownDevice,
    UnsignedDevice,
    UnverifiedIdentity,
    VerificationViolation,
    MismatchedSender,
    SentInClear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shield {
    #[default]
    None,
    Warning(ShieldCode),
    Notice(ShieldCode),
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
            Self::Location(_) => MessagePreviewKind::Location,
            Self::Poll(_) => MessagePreviewKind::Poll,
            Self::UnableToDecrypt(_) => MessagePreviewKind::Encrypted,
        }
    }

    pub fn html(&self) -> Option<&str> {
        match self {
            Self::Text(text) | Self::Notice(text) | Self::Emote(text) => text.html(),
            Self::Image { caption, .. }
            | Self::Video { caption, .. }
            | Self::Audio { caption, .. }
            | Self::File { caption, .. } => caption.as_ref().and_then(RichText::html),
            Self::Sticker { .. }
            | Self::Location(_)
            | Self::Poll(_)
            | Self::Service(_)
            | Self::UnableToDecrypt(_)
            | Self::Unsupported { .. } => None,
        }
    }

    pub fn audio(&self) -> Option<&AudioMeta> {
        match self {
            Self::Audio { meta, .. } => Some(meta),
            _ => None,
        }
    }

    pub fn poll(&self) -> Option<&Poll> {
        match self {
            Self::Poll(poll) => Some(poll),
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
            | Self::Location(_)
            | Self::Poll(_)
            | Self::Service(_)
            | Self::UnableToDecrypt(_)
            | Self::Unsupported { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyInfo {
    pub event_id: String,
    pub sender: NamedUser,
    pub kind: MessagePreviewKind,
    pub body: RichText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedMessage {
    pub event_id: String,
    pub kind: MessagePreviewKind,
    pub body: RichText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinChange {
    Pin,
    Unpin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct MessagePermissions {
    pub send_messages: bool,
    pub react: bool,
    pub send_stickers: bool,
    pub delete_own: bool,
    pub delete_others: bool,
    pub pin: bool,
    pub notify_room: bool,
}

impl MessagePermissions {
    pub const UNRESTRICTED: Self = Self {
        send_messages: true,
        react: true,
        send_stickers: true,
        delete_own: true,
        delete_others: true,
        pin: true,
        notify_room: true,
    };

    pub fn allows_toggle(self, own: Option<ReactionSend>) -> bool {
        match own {
            None => self.react,
            Some(ReactionSend::Sent) => self.delete_own,
            Some(ReactionSend::Sending | ReactionSend::Failed) => true,
        }
    }
}

impl Default for MessagePermissions {
    fn default() -> Self {
        Self::UNRESTRICTED
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
#[allow(clippy::struct_excessive_bools)]
pub struct TimelineMessage {
    pub unique_id: String,
    pub event_id: Option<String>,
    pub local_id: Option<String>,
    pub sender: String,
    pub sender_name: MemberName,
    pub sender_avatar_url: Option<String>,
    pub sender_pronouns: Vec<String>,
    pub body: MessageBody,
    pub body_emoji: BTreeMap<String, CustomEmoji>,
    pub mentions_room: bool,
    pub timestamp: u64,
    pub is_own: bool,
    pub reply: Option<ReplyInfo>,
    pub edited: bool,
    pub editable: bool,
    pub is_first_unread: bool,
    pub send_state: SendState,
    pub shield: Shield,
    pub reactions: Vec<Reaction>,
    pub read_by: ReadBy,
}

impl TimelineMessage {
    pub fn counts_as_unread(&self) -> bool {
        !self.is_own && self.body.service().is_none()
    }

    pub fn editable_text(&self) -> Option<&str> {
        if !self.editable || self.send_state == SendState::Failed {
            return None;
        }
        match &self.body {
            MessageBody::Text(text) => text.composer_text(),
            MessageBody::Image { caption, .. }
            | MessageBody::Video { caption, .. }
            | MessageBody::Audio { caption, .. }
            | MessageBody::File { caption, .. } => {
                caption.as_ref().map_or(Some(""), RichText::composer_text)
            }
            _ => None,
        }
    }

    pub fn tracks_readers(&self) -> bool {
        self.event_id.is_some()
    }

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
