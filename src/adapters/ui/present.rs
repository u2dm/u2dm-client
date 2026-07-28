use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use chrono::{Locale, Timelike};
use pure_rust_locales::locale_match;

use super::schema::{define_ui_enum, message_kinds, service_kinds, verification_phases};
use crate::commands::{UserMessage, UserMessageKind};
use crate::domain::models::{MessageBody, ServiceEvent, TimelineMessage, VerificationCancellation};
use crate::locale::{self, LocaleRequest};

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
        LocaleRequest::from_env(locale::TIME_LOCALE_KEYS)
            .or_else(|| {
                sys_locale::get_locale()
                    .as_deref()
                    .and_then(LocaleRequest::parse)
            })
            .and_then(|request| closest_known_locale(&request))
            .unwrap_or(Locale::POSIX)
    })
}

fn closest_known_locale(request: &LocaleRequest) -> Option<Locale> {
    request
        .candidates()
        .iter()
        .find_map(|candidate| Locale::try_from(candidate.as_str()).ok())
}

fn uses_12h_clock(locale: Locale) -> bool {
    static TWELVE_HOUR: OnceLock<bool> = OnceLock::new();
    *TWELVE_HOUR.get_or_init(|| {
        let time_format = locale_match!(locale => LC_TIME::T_FMT);
        ["%p", "%r", "%I", "%l"]
            .iter()
            .any(|token| time_format.contains(token))
    })
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

thread_local! {
    static ACTIVITY_LABELS: RefCell<HashMap<u64, String>> = RefCell::new(HashMap::new());
}

pub fn invalidate_activity_labels() {
    ACTIVITY_LABELS.with_borrow_mut(HashMap::clear);
}

pub fn room_activity_label(last_activity_ts: u64) -> String {
    if last_activity_ts == 0 {
        return String::new();
    }
    let minute = last_activity_ts / 60_000;
    if let Some(label) = ACTIVITY_LABELS.with_borrow(|labels| labels.get(&minute).cloned()) {
        return label;
    }
    let label = format_activity_label(last_activity_ts);
    ACTIVITY_LABELS.with_borrow_mut(|labels| labels.insert(minute, label.clone()));
    label
}

fn format_activity_label(last_activity_ts: u64) -> String {
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

message_kinds!(define_ui_enum MessageKind;);

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

service_kinds!(define_ui_enum ServiceKind;);

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

verification_phases!(define_ui_enum VerifyStep;);

pub fn verification_cancellation(reason: &VerificationCancellation) -> UserMessage {
    match reason {
        VerificationCancellation::TimedOut => {
            UserMessage::new(UserMessageKind::VerificationTimedOut)
        }
        VerificationCancellation::AcceptFailed => {
            UserMessage::new(UserMessageKind::VerificationSasAcceptFailed)
        }
        VerificationCancellation::Remote => {
            UserMessage::new(UserMessageKind::VerificationCancelled)
        }
    }
}
