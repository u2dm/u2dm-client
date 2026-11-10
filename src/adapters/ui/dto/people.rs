use std::path::PathBuf;

use slint::{Image, SharedString};

use crate::adapters::ui::decode::{AvatarSlot, load_avatar_async, peek_avatar};
use crate::adapters::ui::present::{
    avatar_color_index, name_initials, pronoun_labels, room_activity_label,
};
use crate::adapters::ui::schema::{define_ui_enum, member_row_kinds};
use crate::commands::messages::UserMessageKind;
use crate::commands::view::{IgnoredPerson, InviteeRow, InviteeState, RosterRow};
use crate::domain::member_name::{MemberName, isolated};
use crate::domain::room_info::{MemberRole, Reader, RosterMember};
use crate::domain::sessions::OwnSession;
use crate::domain::user_info::{DeviceTrust, Pronouns, UserDevice, UserProfile};
use crate::ports::media::MediaCache;

member_row_kinds!(define_ui_enum MemberRowKind;);

pub const INVITED_HEADING_ROW: &str = "invited-heading";

const PRONOUN_SEPARATOR: &str = " · ";

pub struct MemberRowDto {
    pub user_id: SharedString,
    pub kind: MemberRowKind,
    pub name: SharedString,
    pub initial: SharedString,
    pub color_index: i32,
    pub role: MemberRole,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
}

pub struct InviteeDto {
    pub user_id: SharedString,
    pub name: SharedString,
    pub initial: SharedString,
    pub color_index: i32,
    pub state: InviteeState,
    pub failure: UserMessageKind,
    pub failure_detail: SharedString,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
}

pub struct IgnoredPersonDto {
    pub user_id: SharedString,
    pub name: SharedString,
    pub busy: bool,
}

pub struct UserDeviceDto {
    pub device_id: SharedString,
    pub name: SharedString,
    pub trust: DeviceTrust,
    pub last_seen: SharedString,
    pub ip: SharedString,
}

pub fn ignored_person_to_dto(person: &IgnoredPerson) -> IgnoredPersonDto {
    IgnoredPersonDto {
        user_id: SharedString::from(person.user_id.as_ref()),
        name: bare_name(&person.name),
        busy: person.busy,
    }
}

fn bare_name(name: &MemberName) -> SharedString {
    name.set()
        .map(|name| SharedString::from(isolated(name)))
        .unwrap_or_default()
}

pub fn member_row_to_dto(row: &RosterRow, media: &dyn MediaCache) -> MemberRowDto {
    let RosterRow::Member(member) = row else {
        return MemberRowDto {
            user_id: SharedString::from(INVITED_HEADING_ROW),
            kind: MemberRowKind::InvitedHeading,
            name: SharedString::new(),
            initial: SharedString::new(),
            color_index: 0,
            role: MemberRole::Member,
            avatar: None,
            has_avatar: false,
        };
    };
    let avatar = member_avatar_path(member, media).and_then(|path| peek_avatar(&path));
    person_row(&member.user_id, &member.name, member.role, avatar)
}

fn person_row(
    user_id: &str,
    name: &MemberName,
    role: MemberRole,
    avatar: Option<Image>,
) -> MemberRowDto {
    MemberRowDto {
        user_id: SharedString::from(user_id),
        kind: MemberRowKind::Member,
        name: bare_name(name),
        initial: SharedString::from(name_initials(name, user_id)),
        color_index: avatar_color_index(user_id),
        role,
        has_avatar: avatar.is_some(),
        avatar,
    }
}

pub fn reader_to_dto(reader: &Reader, media: &dyn MediaCache) -> MemberRowDto {
    let avatar = reader_avatar_path(reader, media).and_then(|path| peek_avatar(&path));
    person_row(&reader.user_id, &reader.name, reader.role, avatar)
}

pub fn request_reader_avatar(reader: &Reader, media: &dyn MediaCache) {
    let path = reader_avatar_path(reader, media);
    load_avatar_async(path.as_deref(), AvatarSlot::Reader(reader.user_id.clone()));
}

fn reader_avatar_path(reader: &Reader, media: &dyn MediaCache) -> Option<PathBuf> {
    media.user_avatar_path(reader.avatar_mxc.as_deref()?)
}

pub fn mention_to_dto(member: &RosterMember, media: &dyn MediaCache) -> MemberRowDto {
    let avatar = member_avatar_path(member, media).and_then(|path| peek_avatar(&path));
    person_row(&member.user_id, &member.name, member.role, avatar)
}

pub fn request_mention_avatar(member: &RosterMember, media: &dyn MediaCache) {
    let path = member_avatar_path(member, media);
    load_avatar_async(path.as_deref(), AvatarSlot::Mention(member.user_id.clone()));
}

pub fn invitee_to_dto(row: &InviteeRow, media: &dyn MediaCache) -> InviteeDto {
    let person = &row.person;
    let avatar = invitee_avatar_path(row, media).and_then(|path| peek_avatar(&path));
    InviteeDto {
        user_id: SharedString::from(person.user_id.as_ref()),
        name: bare_name(&person.name),
        initial: SharedString::from(name_initials(&person.name, &person.user_id)),
        color_index: avatar_color_index(&person.user_id),
        state: row.state,
        failure: row.failure.kind,
        failure_detail: SharedString::from(row.failure.detail.as_str()),
        has_avatar: avatar.is_some(),
        avatar,
    }
}

pub fn request_invitee_avatar(row: &InviteeRow, media: &dyn MediaCache) {
    let path = invitee_avatar_path(row, media);
    load_avatar_async(
        path.as_deref(),
        AvatarSlot::Invitee(row.person.user_id.to_string()),
    );
}

fn invitee_avatar_path(row: &InviteeRow, media: &dyn MediaCache) -> Option<PathBuf> {
    media.user_avatar_path(row.person.avatar_mxc.as_deref()?)
}

pub fn user_device_to_dto(device: &UserDevice) -> UserDeviceDto {
    UserDeviceDto {
        device_id: SharedString::from(device.device_id.as_str()),
        name: SharedString::from(device.name.as_deref().unwrap_or_default()),
        trust: device.trust,
        last_seen: SharedString::new(),
        ip: SharedString::new(),
    }
}

pub fn own_session_to_dto(session: &OwnSession) -> UserDeviceDto {
    UserDeviceDto {
        device_id: SharedString::from(session.device_id.as_str()),
        name: SharedString::from(session.name.as_deref().unwrap_or_default()),
        trust: session.trust,
        last_seen: session.last_seen_ms.map_or_else(SharedString::new, |at| {
            SharedString::from(room_activity_label(at))
        }),
        ip: SharedString::from(session.last_seen_ip.as_deref().unwrap_or_default()),
    }
}

pub fn request_member_avatar(row: &RosterRow, media: &dyn MediaCache) {
    let RosterRow::Member(member) = row else {
        return;
    };
    let path = member_avatar_path(member, media);
    load_avatar_async(path.as_deref(), AvatarSlot::Member(member.user_id.clone()));
}

fn member_avatar_path(member: &RosterMember, media: &dyn MediaCache) -> Option<PathBuf> {
    media.user_avatar_path(member.avatar_mxc.as_deref()?)
}

pub fn load_user_info_avatar(profile: &UserProfile, media: &dyn MediaCache) -> Option<Image> {
    let path = profile
        .avatar_mxc
        .as_deref()
        .and_then(|mxc| media.user_avatar_path(mxc));
    load_avatar_async(path.as_deref(), AvatarSlot::UserInfo)
}

pub fn user_info_pronouns(profile: &UserProfile) -> SharedString {
    match &profile.pronouns {
        Pronouns::Known(pronouns) => {
            SharedString::from(pronoun_labels(pronouns).join(PRONOUN_SEPARATOR))
        }
        Pronouns::Unknown => SharedString::new(),
    }
}
