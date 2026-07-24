use crate::domain::models::{
    LoginMethod, MessageBody, MessagePreviewKind, ServiceEvent, TimelineMessage,
};

pub fn sender_initial(name: &str) -> &str {
    match name.chars().next() {
        Some(c) => &name[..c.len_utf8()],
        None => "",
    }
}

const AVATAR_COLORS: u32 = 7;

pub fn avatar_initials(name: &str) -> String {
    let initials: String = name
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();

    if initials.is_empty() {
        return sender_initial(name.trim()).to_owned();
    }
    initials
}

pub fn avatar_color_index(id: &str) -> i32 {
    let hash = id.bytes().fold(0_u32, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(u32::from(byte))
    });
    i32::try_from(hash % AVATAR_COLORS).unwrap_or_default()
}

pub fn user_initial(user_id: &str) -> String {
    let name = user_id.strip_prefix('@').unwrap_or(user_id);
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
}

pub fn message_sender_label(m: &TimelineMessage) -> &str {
    m.sender_display_name.as_deref().unwrap_or(&m.sender)
}

const PRONOUNS_SHOWN: usize = 3;
const PRONOUNS_MAX_LEN: usize = 16;

pub fn pronoun_labels(pronouns: &[String]) -> Vec<String> {
    pronouns
        .iter()
        .map(|set| set.trim())
        .filter(|set| !set.is_empty())
        .take(PRONOUNS_SHOWN)
        .map(|set| truncate_chars(set, PRONOUNS_MAX_LEN).to_lowercase())
        .collect()
}

fn truncate_chars(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((cut, _)) => text.get(..cut).unwrap_or(text),
        None => text,
    }
}

pub fn message_timestamp_label(timestamp: u64) -> String {
    chrono::DateTime::from_timestamp((timestamp / 1000).cast_signed(), 0)
        .map(|utc| {
            utc.with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

pub fn room_activity_label(last_activity_ts: u64) -> String {
    if last_activity_ts == 0 {
        return String::new();
    }
    let Some(utc) = chrono::DateTime::from_timestamp((last_activity_ts / 1000).cast_signed(), 0)
    else {
        return String::new();
    };
    let local = utc.with_timezone(&chrono::Local);
    let days = chrono::Local::now()
        .date_naive()
        .signed_duration_since(local.date_naive())
        .num_days();
    if days <= 0 {
        local.format("%H:%M").to_string()
    } else if days < 7 {
        local.format("%a").to_string()
    } else {
        local.format("%d/%m/%y").to_string()
    }
}

pub fn message_body_text(body: &MessageBody) -> &str {
    match body {
        MessageBody::Text(s) | MessageBody::Notice(s) | MessageBody::Emote(s) => s,
        MessageBody::Image { caption, .. } => caption.as_deref().unwrap_or_default(),
        MessageBody::File { meta, .. } => &meta.filename,
        MessageBody::Service(_) | MessageBody::UnableToDecrypt => "",
        MessageBody::Unsupported { fallback, .. } => fallback,
    }
}

#[derive(Clone, Copy)]
pub enum MessageKind {
    Text,
    Notice,
    Emote,
    Image,
    File,
    Service,
    Utd,
    Unsupported,
}

impl MessageKind {
    #[cfg(feature = "interpreted")]
    pub fn slint(self) -> (&'static str, &'static str) {
        let variant = match self {
            Self::Text => "text",
            Self::Notice => "notice",
            Self::Emote => "emote",
            Self::Image => "image",
            Self::File => "file",
            Self::Service => "service",
            Self::Utd => "utd",
            Self::Unsupported => "unsupported",
        };
        ("MessageKind", variant)
    }
}

pub fn message_kind(body: &MessageBody) -> MessageKind {
    match body {
        MessageBody::Text(_) => MessageKind::Text,
        MessageBody::Notice(_) => MessageKind::Notice,
        MessageBody::Emote(_) => MessageKind::Emote,
        MessageBody::Image { .. } => MessageKind::Image,
        MessageBody::File { .. } => MessageKind::File,
        MessageBody::Service(_) => MessageKind::Service,
        MessageBody::UnableToDecrypt => MessageKind::Utd,
        MessageBody::Unsupported { .. } => MessageKind::Unsupported,
    }
}

#[derive(Clone, Copy)]
pub enum ServiceKind {
    None,
    Joined,
    Left,
    Invited,
    InvitationAccepted,
    InvitationRejected,
    InvitationRevoked,
    Kicked,
    Banned,
    Unbanned,
    Knocked,
    KnockAccepted,
    NameSet,
    NameChanged,
    NameRemoved,
    AvatarChanged,
    RoomName,
    RoomTopic,
    RoomAvatar,
    RoomCreated,
    Encryption,
    CallStarted,
    CallNotification,
}

impl ServiceKind {
    #[cfg(feature = "interpreted")]
    pub fn slint(self) -> (&'static str, &'static str) {
        let variant = match self {
            Self::None => "none",
            Self::Joined => "joined",
            Self::Left => "left",
            Self::Invited => "invited",
            Self::InvitationAccepted => "invitation-accepted",
            Self::InvitationRejected => "invitation-rejected",
            Self::InvitationRevoked => "invitation-revoked",
            Self::Kicked => "kicked",
            Self::Banned => "banned",
            Self::Unbanned => "unbanned",
            Self::Knocked => "knocked",
            Self::KnockAccepted => "knock-accepted",
            Self::NameSet => "name-set",
            Self::NameChanged => "name-changed",
            Self::NameRemoved => "name-removed",
            Self::AvatarChanged => "avatar-changed",
            Self::RoomName => "room-name",
            Self::RoomTopic => "room-topic",
            Self::RoomAvatar => "room-avatar",
            Self::RoomCreated => "room-created",
            Self::Encryption => "encryption",
            Self::CallStarted => "call-started",
            Self::CallNotification => "call-notification",
        };
        ("ServiceKind", variant)
    }
}

pub fn service_kind(event: &ServiceEvent) -> ServiceKind {
    match event {
        ServiceEvent::Joined => ServiceKind::Joined,
        ServiceEvent::Left => ServiceKind::Left,
        ServiceEvent::Invited { .. } => ServiceKind::Invited,
        ServiceEvent::InvitationAccepted => ServiceKind::InvitationAccepted,
        ServiceEvent::InvitationRejected => ServiceKind::InvitationRejected,
        ServiceEvent::InvitationRevoked { .. } => ServiceKind::InvitationRevoked,
        ServiceEvent::Kicked { .. } => ServiceKind::Kicked,
        ServiceEvent::Banned { .. } => ServiceKind::Banned,
        ServiceEvent::Unbanned { .. } => ServiceKind::Unbanned,
        ServiceEvent::Knocked => ServiceKind::Knocked,
        ServiceEvent::KnockAccepted { .. } => ServiceKind::KnockAccepted,
        ServiceEvent::DisplayNameSet { .. } => ServiceKind::NameSet,
        ServiceEvent::DisplayNameChanged { .. } => ServiceKind::NameChanged,
        ServiceEvent::DisplayNameRemoved => ServiceKind::NameRemoved,
        ServiceEvent::AvatarChanged => ServiceKind::AvatarChanged,
        ServiceEvent::RoomNameChanged { .. } => ServiceKind::RoomName,
        ServiceEvent::RoomTopicChanged => ServiceKind::RoomTopic,
        ServiceEvent::RoomAvatarChanged => ServiceKind::RoomAvatar,
        ServiceEvent::RoomCreated => ServiceKind::RoomCreated,
        ServiceEvent::EncryptionEnabled => ServiceKind::Encryption,
        ServiceEvent::CallStarted => ServiceKind::CallStarted,
        ServiceEvent::CallNotification => ServiceKind::CallNotification,
    }
}

pub fn service_target(event: &ServiceEvent) -> &str {
    match event {
        ServiceEvent::Invited { target }
        | ServiceEvent::InvitationRevoked { target }
        | ServiceEvent::Kicked { target }
        | ServiceEvent::Banned { target }
        | ServiceEvent::Unbanned { target }
        | ServiceEvent::KnockAccepted { target } => target.as_deref().unwrap_or_default(),
        ServiceEvent::DisplayNameSet { name }
        | ServiceEvent::DisplayNameChanged { name }
        | ServiceEvent::RoomNameChanged { name } => name,
        _ => "",
    }
}

pub fn unsupported_kind(body: &MessageBody) -> &str {
    match body {
        MessageBody::Unsupported { kind, .. } => kind,
        _ => "",
    }
}

#[derive(Clone, Copy)]
pub enum PreviewKind {
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

impl PreviewKind {
    #[cfg(feature = "interpreted")]
    pub fn slint(self) -> (&'static str, &'static str) {
        let variant = match self {
            Self::None => "none",
            Self::Text => "text",
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::File => "file",
            Self::Location => "location",
            Self::Encrypted => "encrypted",
            Self::Sticker => "sticker",
        };
        ("PreviewKind", variant)
    }
}

pub fn preview_kind(kind: MessagePreviewKind) -> PreviewKind {
    match kind {
        MessagePreviewKind::None => PreviewKind::None,
        MessagePreviewKind::Text => PreviewKind::Text,
        MessagePreviewKind::Image => PreviewKind::Image,
        MessagePreviewKind::Video => PreviewKind::Video,
        MessagePreviewKind::Audio => PreviewKind::Audio,
        MessagePreviewKind::File => PreviewKind::File,
        MessagePreviewKind::Location => PreviewKind::Location,
        MessagePreviewKind::Encrypted => PreviewKind::Encrypted,
        MessagePreviewKind::Sticker => PreviewKind::Sticker,
    }
}

#[derive(Clone, Copy)]
pub enum LoginMethodKind {
    None,
    Password,
    OAuth,
    Both,
}

impl LoginMethodKind {
    #[cfg(feature = "interpreted")]
    pub fn slint(self) -> (&'static str, &'static str) {
        let variant = match self {
            Self::None => "none",
            Self::Password => "password",
            Self::OAuth => "oauth",
            Self::Both => "both",
        };
        ("LoginMethodKind", variant)
    }
}

pub fn login_method_kind(method: LoginMethod) -> LoginMethodKind {
    match method {
        LoginMethod::Password => LoginMethodKind::Password,
        LoginMethod::OAuth => LoginMethodKind::OAuth,
        LoginMethod::Both => LoginMethodKind::Both,
        LoginMethod::None => LoginMethodKind::None,
    }
}

pub enum Status {
    CheckingServer,
    LoggingIn,
    OpeningBrowser,
    FileSaved,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CheckingServer => "checking-server",
            Self::LoggingIn => "logging-in",
            Self::OpeningBrowser => "opening-browser",
            Self::FileSaved => "file-saved",
        }
    }
}

#[derive(Clone, Copy)]
pub enum VerifyStep {
    None,
    Requested,
    Emojis,
    Confirming,
    Done,
    Cancelled,
}
