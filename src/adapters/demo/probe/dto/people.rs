use serde::Serialize;

use super::{MessageDto, message};
use crate::adapters::demo::probe::names;
use crate::commands::view::{
    CardStatus, MentionsView, PresenceView, ReadersView, TypingView, UserInfoView,
};
use crate::domain::presence::PresenceLine;
use crate::domain::room_info::Reader;
use crate::domain::user_info::{Moderation, Pronouns};

#[derive(Serialize)]
pub(super) struct TypingDto {
    room_id: Option<String>,
    typists: Vec<TypistDto>,
}

#[derive(Serialize)]
struct TypistDto {
    user_id: String,
    shown: String,
}

#[derive(Serialize)]
pub(super) struct PresenceDto {
    partner: Option<PartnerPresenceDto>,
    person: Option<PersonPresenceDto>,
}

#[derive(Serialize)]
struct PartnerPresenceDto {
    room_id: String,
    user_id: String,
    line: &'static str,
    minutes: Option<u32>,
}

#[derive(Serialize)]
struct PersonPresenceDto {
    user_id: String,
    line: &'static str,
    minutes: Option<u32>,
    status: Option<String>,
}

#[derive(Serialize)]
pub(super) struct MentionsDto {
    room_id: Option<String>,
    offers_room: bool,
    avatars_ready: usize,
    rows: Vec<MentionDto>,
}

#[derive(Serialize)]
struct MentionDto {
    user_id: String,
    name: String,
}

#[derive(Serialize)]
pub(super) struct ReadersDto {
    status: &'static str,
    event_id: Option<String>,
    sender: Option<String>,
    kind: Option<&'static str>,
    total: usize,
    has_more: bool,
    pages_landed: i32,
    avatars_ready: usize,
    rows: Vec<ReaderDto>,
}

#[derive(Serialize)]
struct ReaderDto {
    user_id: String,
    name: String,
    role: &'static str,
    has_avatar_mxc: bool,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub(super) struct UserInfoDto {
    open: bool,
    status: Option<&'static str>,
    room_id: Option<String>,
    user_id: Option<String>,
    name: Option<String>,
    has_avatar_mxc: bool,
    pronouns: Option<Vec<String>>,
    role: Option<&'static str>,
    membership: Option<&'static str>,
    identity: &'static str,
    is_self: bool,
    link: Option<String>,
    direct_room: Option<String>,
    direct: &'static str,
    verify: &'static str,
    sessions: Option<Vec<UserDeviceDto>>,
    ignored: bool,
    ignore_busy: bool,
    may_kick: bool,
    may_ban: bool,
    may_unban: bool,
    moderating: &'static str,
    avatars_ready: usize,
    error: MessageDto,
}

#[derive(Serialize)]
struct UserDeviceDto {
    device_id: String,
    name: Option<String>,
    trust: &'static str,
}

pub(super) fn typing(source: &TypingView) -> TypingDto {
    TypingDto {
        room_id: source.room_id.as_ref().map(ToString::to_string),
        typists: source
            .typists
            .users()
            .iter()
            .map(|user| TypistDto {
                user_id: user.user_id.clone(),
                shown: user.shown(),
            })
            .collect(),
    }
}

pub(super) fn presence(source: &PresenceView) -> PresenceDto {
    PresenceDto {
        partner: source.partner.as_ref().map(|partner| PartnerPresenceDto {
            room_id: partner.room_id.to_string(),
            user_id: partner.user_id.to_string(),
            line: names::presence_line(partner.line),
            minutes: minutes_seen(partner.line),
        }),
        person: source.person.as_ref().map(|person| PersonPresenceDto {
            user_id: person.user_id.to_string(),
            line: names::presence_line(person.line),
            minutes: minutes_seen(person.line),
            status: person.status.clone(),
        }),
    }
}

const fn minutes_seen(line: PresenceLine) -> Option<u32> {
    match line {
        PresenceLine::LastSeen { minutes } => Some(minutes),
        PresenceLine::Hidden | PresenceLine::Online | PresenceLine::Away => None,
    }
}

pub(super) fn mentions(source: &MentionsView) -> MentionsDto {
    MentionsDto {
        room_id: source.room_id.as_ref().map(ToString::to_string),
        offers_room: source.offers_room,
        avatars_ready: source.avatars_ready,
        rows: source
            .rows
            .iter()
            .map(|member| MentionDto {
                user_id: member.user_id.clone(),
                name: member.name.shown(&member.user_id),
            })
            .collect(),
    }
}

pub(super) fn readers(source: &ReadersView) -> ReadersDto {
    ReadersDto {
        status: names::readers_status(source.status),
        event_id: source
            .message
            .as_ref()
            .and_then(|message| message.event_id.clone()),
        sender: source
            .message
            .as_ref()
            .map(|message| message.sender.clone()),
        kind: source
            .message
            .as_ref()
            .map(|message| names::preview_kind(message.body.preview_kind())),
        total: source.total,
        has_more: source.has_more,
        pages_landed: source.pages_landed,
        avatars_ready: source.avatars_ready,
        rows: source
            .rows
            .iter()
            .map(|reader| reader_row(reader))
            .collect(),
    }
}

fn reader_row(source: &Reader) -> ReaderDto {
    ReaderDto {
        user_id: source.user_id.clone(),
        name: source.name.shown(&source.user_id),
        role: names::member_role(source.role),
        has_avatar_mxc: source.avatar_mxc.is_some(),
    }
}

pub(super) fn user_info(source: &UserInfoView) -> UserInfoDto {
    let card = source.card.as_ref();
    let profile = card.map(|card| &card.profile);
    UserInfoDto {
        open: card.is_some(),
        status: card.map(|card| card_status(card.status)),
        room_id: card.map(|card| card.room_id.to_string()),
        user_id: profile.map(|profile| profile.user_id.to_string()),
        name: profile.and_then(|profile| profile.name.set().map(str::to_owned)),
        has_avatar_mxc: profile.is_some_and(|profile| profile.avatar_mxc.is_some()),
        pronouns: profile.and_then(|profile| match &profile.pronouns {
            Pronouns::Known(pronouns) => Some(pronouns.clone()),
            Pronouns::Unknown => None,
        }),
        role: profile.map(|profile| names::member_role(profile.role)),
        membership: profile.map(|profile| names::room_membership(profile.membership)),
        identity: profile.map_or("unverified", |profile| names::identity_trust(profile.trust)),
        is_self: profile.is_some_and(|profile| profile.is_self),
        link: profile.map(|profile| profile.link.clone()),
        direct_room: profile
            .and_then(|profile| profile.direct_room.as_ref().map(ToString::to_string)),
        direct: names::direct_chat(source.direct),
        verify: names::verify_offer(source.verify),
        sessions: source.sessions.as_ref().map(|devices| {
            devices
                .iter()
                .map(|device| UserDeviceDto {
                    device_id: device.device_id.clone(),
                    name: device.name.clone(),
                    trust: names::device_trust(device.trust),
                })
                .collect()
        }),
        ignored: profile.is_some_and(|profile| profile.ignored),
        ignore_busy: source.ignore_busy,
        may_kick: profile.is_some_and(|profile| profile.offers(Moderation::Kick)),
        may_ban: profile.is_some_and(|profile| profile.offers(Moderation::Ban)),
        may_unban: profile.is_some_and(|profile| profile.offers(Moderation::Unban)),
        moderating: names::pending_moderation(source.moderating),
        avatars_ready: source.avatars_ready,
        error: message(&source.error),
    }
}

fn card_status(status: CardStatus) -> &'static str {
    match status {
        CardStatus::Loaded => "loaded",
        CardStatus::ReadFailed => "read-failed",
        CardStatus::Retrying => "retrying",
    }
}
