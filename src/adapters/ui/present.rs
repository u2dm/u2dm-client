use std::env;
use std::sync::OnceLock;

use chrono::{Locale, Timelike};
use pure_rust_locales::locale_match;

use crate::commands::LoginStatus;
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

fn active_locale() -> Locale {
    static LOCALE: OnceLock<Locale> = OnceLock::new();
    *LOCALE.get_or_init(|| {
        lc_time_locale()
            .or_else(sys_locale::get_locale)
            .as_deref()
            .and_then(parse_locale)
            .unwrap_or(Locale::POSIX)
    })
}

fn lc_time_locale() -> Option<String> {
    ["LC_ALL", "LC_TIME", "LANG"]
        .into_iter()
        .find_map(|key| env::var(key).ok().filter(|value| !value.is_empty()))
}

fn parse_locale(tag: &str) -> Option<Locale> {
    let core = tag
        .split(['.', '@'])
        .next()
        .unwrap_or(tag)
        .replace('-', "_");
    let mut candidate: &str = &core;
    loop {
        if let Ok(locale) = Locale::try_from(candidate) {
            return Some(locale);
        }
        let cut = candidate.rfind('_')?;
        candidate = candidate.get(..cut)?;
    }
}

fn uses_12h_clock(locale: Locale) -> bool {
    let time_format = locale_match!(locale => LC_TIME::T_FMT);
    ["%p", "%r", "%I", "%l"]
        .iter()
        .any(|token| time_format.contains(token))
}

fn to_local(timestamp_ms: u64) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::from_timestamp((timestamp_ms / 1000).cast_signed(), 0)
        .map(|utc| utc.with_timezone(&chrono::Local))
}

fn short_time(local: &chrono::DateTime<chrono::Local>, locale: Locale) -> String {
    let minute = local.minute();
    if uses_12h_clock(locale) {
        let (_, hour) = local.hour12();
        let period = local.format_localized("%p", locale).to_string();
        format!("{hour}:{minute:02} {period}")
    } else {
        let hour = local.hour();
        format!("{hour:02}:{minute:02}")
    }
}

pub fn message_timestamp_label(timestamp: u64) -> String {
    to_local(timestamp)
        .map(|local| short_time(&local, active_locale()))
        .unwrap_or_default()
}

pub fn room_activity_label(last_activity_ts: u64) -> String {
    if last_activity_ts == 0 {
        return String::new();
    }
    let Some(local) = to_local(last_activity_ts) else {
        return String::new();
    };
    let locale = active_locale();
    let days = chrono::Local::now()
        .date_naive()
        .signed_duration_since(local.date_naive())
        .num_days();
    if days <= 0 {
        short_time(&local, locale)
    } else if days < 7 {
        local.format_localized("%a", locale).to_string()
    } else {
        local.format_localized("%x", locale).to_string()
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

pub const FILE_SAVED_TOAST: &str = "file-saved";

pub fn login_status_token(status: LoginStatus) -> &'static str {
    match status {
        LoginStatus::Idle => "",
        LoginStatus::LoadingSession => "loading-session",
        LoginStatus::OpeningStore => "opening-store",
        LoginStatus::Connecting => "connecting",
        LoginStatus::RestoringAuth => "restoring-auth",
        LoginStatus::CheckingServer => "checking-server",
        LoginStatus::LoggingIn => "logging-in",
        LoginStatus::OpeningBrowser => "opening-browser",
        LoginStatus::WaitingAuth => "waiting-auth",
        LoginStatus::Syncing => "syncing",
        LoginStatus::CleaningUp => "cleaning-up",
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
