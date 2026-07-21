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

pub fn message_type_token(body: &MessageBody) -> &'static str {
    match body {
        MessageBody::Text(_) => "text",
        MessageBody::Notice(_) => "notice",
        MessageBody::Emote(_) => "emote",
        MessageBody::Image { .. } => "image",
        MessageBody::File { .. } => "file",
        MessageBody::Service(_) => "service",
        MessageBody::UnableToDecrypt => "utd",
        MessageBody::Unsupported { .. } => "unsupported",
    }
}

pub fn service_kind_token(event: &ServiceEvent) -> &'static str {
    match event {
        ServiceEvent::Joined => "joined",
        ServiceEvent::Left => "left",
        ServiceEvent::Invited { .. } => "invited",
        ServiceEvent::InvitationAccepted => "invitation-accepted",
        ServiceEvent::InvitationRejected => "invitation-rejected",
        ServiceEvent::InvitationRevoked { .. } => "invitation-revoked",
        ServiceEvent::Kicked { .. } => "kicked",
        ServiceEvent::Banned { .. } => "banned",
        ServiceEvent::Unbanned { .. } => "unbanned",
        ServiceEvent::Knocked => "knocked",
        ServiceEvent::KnockAccepted { .. } => "knock-accepted",
        ServiceEvent::DisplayNameSet { .. } => "name-set",
        ServiceEvent::DisplayNameChanged { .. } => "name-changed",
        ServiceEvent::DisplayNameRemoved => "name-removed",
        ServiceEvent::AvatarChanged => "avatar-changed",
        ServiceEvent::RoomNameChanged { .. } => "room-name",
        ServiceEvent::RoomTopicChanged => "room-topic",
        ServiceEvent::RoomAvatarChanged => "room-avatar",
        ServiceEvent::RoomCreated => "room-created",
        ServiceEvent::EncryptionEnabled => "encryption",
        ServiceEvent::CallStarted => "call-started",
        ServiceEvent::CallNotification => "call-notification",
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

pub fn message_preview_kind_token(kind: MessagePreviewKind) -> &'static str {
    match kind {
        MessagePreviewKind::None => "",
        MessagePreviewKind::Text => "text",
        MessagePreviewKind::Image => "image",
        MessagePreviewKind::Video => "video",
        MessagePreviewKind::Audio => "audio",
        MessagePreviewKind::File => "file",
        MessagePreviewKind::Location => "location",
        MessagePreviewKind::Encrypted => "encrypted",
        MessagePreviewKind::Sticker => "sticker",
    }
}

pub fn login_method_token(method: LoginMethod) -> &'static str {
    match method {
        LoginMethod::Password => "password",
        LoginMethod::OAuth => "oauth",
        LoginMethod::Both => "both",
        LoginMethod::None => "",
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
