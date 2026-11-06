use std::path::PathBuf;

use slint::{Image, SharedString};

use super::body::preview_line;
use crate::adapters::ui::decode::{
    AvatarSlot, ShownAvatar, load_avatar_async, peek_avatar, record_avatar_need,
};
use crate::adapters::ui::present::{
    ServiceKind, avatar_color_index, avatar_initials, log_level_name, log_time_label,
    room_activity_label, sender_initial, service_kind, service_target,
};
use crate::adapters::ui::schema::{define_ui_enum, room_call_marks, room_row_kinds};
use crate::commands::view::{ChildAccess, RoomCard, SpaceIndexRow, SpaceMatch};
use crate::domain::departure::{Removal, RemovalKind};
use crate::domain::invite::{Invite, InviteKind};
use crate::domain::member_name::isolated;
use crate::domain::message::MessagePreviewKind;
use crate::domain::room::{Room, Space};
use crate::domain::room_log::{LogLevel, LogLine};
use crate::domain::room_preview::RoomPreview;
use crate::domain::space_index::{ChildKind, SpaceChild};
use crate::ports::media::MediaCache;
use crate::util::saturating_i32;

room_row_kinds!(define_ui_enum RoomRowKind;);

room_call_marks!(define_ui_enum RoomCallMark;);

pub const INVITES_HEADING_ROW: &str = "invites-heading";

pub const ROOMS_HEADING_ROW: &str = "rooms-heading";

pub const SPACES_HEADING_ROW: &str = "spaces-heading";

#[allow(clippy::struct_excessive_bools)]
pub struct RoomDto {
    pub id: SharedString,
    pub kind: RoomRowKind,
    pub name: SharedString,
    pub initial: SharedString,
    pub color_index: i32,
    pub members: i32,
    pub alert: bool,
    pub mention: bool,
    pub hint: bool,
    pub muted: bool,
    pub last_message_sender: SharedString,
    pub last_message_kind: MessagePreviewKind,
    pub last_message_body: SharedString,
    pub last_message_service_kind: ServiceKind,
    pub last_message_service_target: SharedString,
    pub last_message_is_own: bool,
    pub last_message_edited: bool,
    pub last_message_time: SharedString,
    pub space_rooms: i32,
    pub space_parent: SharedString,
    pub invite_count: i32,
    pub invite_kind: InviteKind,
    pub inviter: SharedString,
    pub inviter_id: SharedString,
    pub removal: RemovalKind,
    pub removed_by: SharedString,
    pub call: RoomCallMark,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
}

#[allow(clippy::struct_excessive_bools)]
pub struct SpaceDto {
    pub id: SharedString,
    pub name: SharedString,
    pub alert: bool,
    pub mention: bool,
    pub hint: bool,
    pub initial: SharedString,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
}

pub struct SpaceChildDto {
    pub id: SharedString,
    pub name: SharedString,
    pub initial: SharedString,
    pub color_index: i32,
    pub detail: SharedString,
    pub members: i32,
    pub is_space: bool,
    pub children: i32,
    pub access: ChildAccess,
    pub avatar: Option<Image>,
    pub has_avatar: bool,
}

pub struct LogLineDto {
    pub time: SharedString,
    pub level: LogLevel,
    pub level_name: SharedString,
    pub target: SharedString,
    pub text: SharedString,
}

pub fn room_to_dto(r: &Room, media: &dyn MediaCache) -> RoomDto {
    let avatar = r
        .avatar_mxc
        .as_deref()
        .and_then(|mxc| media.room_avatar_path(mxc))
        .and_then(|path| peek_avatar(&path));
    RoomDto {
        name: SharedString::from(&r.display_name),
        initial: SharedString::from(avatar_initials(&r.display_name)),
        color_index: avatar_color_index(r.id.as_ref()),
        members: if r.is_direct {
            0
        } else {
            saturating_i32(r.member_count)
        },
        alert: r.alert(),
        mention: r.mention(),
        hint: r.hint(),
        muted: r.muted(),
        last_message_sender: r
            .last_message_sender
            .as_ref()
            .map(|sender| SharedString::from(sender.shown()))
            .unwrap_or_default(),
        last_message_kind: r.last_message_kind,
        last_message_body: preview_line(&r.last_message_body),
        last_message_service_kind: r
            .last_message_service
            .as_ref()
            .map_or(ServiceKind::None, service_kind),
        last_message_service_target: SharedString::from(
            r.last_message_service
                .as_ref()
                .map(service_target)
                .unwrap_or_default(),
        ),
        last_message_is_own: r.last_message_is_own,
        last_message_edited: r.last_message_edited,
        last_message_time: SharedString::from(&room_activity_label(r.last_activity_ts)),
        call: call_mark(r),
        has_avatar: avatar.is_some(),
        avatar,
        ..bare_row_dto(r.id.as_ref(), RoomRowKind::Room)
    }
}

fn call_mark(room: &Room) -> RoomCallMark {
    match room.call.as_deref() {
        None => RoomCallMark::None,
        Some(call) if call.is_video() => RoomCallMark::Video,
        Some(_) => RoomCallMark::Voice,
    }
}

pub fn record_room_avatar_need(r: &Room, media: &dyn MediaCache) -> ShownAvatar {
    let avatar_path = r
        .avatar_mxc
        .as_deref()
        .and_then(|mxc| media.room_avatar_path(mxc));
    record_avatar_need(
        &AvatarSlot::Room(r.id.as_ref().to_owned()),
        avatar_path.as_deref(),
    )
}

pub fn heading_to_dto(id: &str, kind: RoomRowKind, invites: usize) -> RoomDto {
    RoomDto {
        invite_count: saturating_i32(invites),
        ..bare_row_dto(id, kind)
    }
}

pub fn invite_to_dto(invite: &Invite, media: &dyn MediaCache) -> RoomDto {
    let avatar = invite_avatar_path(invite, media).and_then(|path| peek_avatar(&path));
    let inviter = invite.inviter.as_ref();
    RoomDto {
        name: SharedString::from(&invite.name),
        initial: SharedString::from(avatar_initials(&invite.name)),
        color_index: avatar_color_index(invite.id.as_ref()),
        alert: true,
        last_message_time: SharedString::from(&room_activity_label(invite.received_ts)),
        invite_kind: invite.kind,
        inviter: inviter
            .map(|inviter| SharedString::from(inviter.shown()))
            .unwrap_or_default(),
        inviter_id: inviter
            .map(|inviter| SharedString::from(isolated(&inviter.user_id)))
            .unwrap_or_default(),
        has_avatar: avatar.is_some(),
        avatar,
        ..bare_row_dto(invite.id.as_ref(), RoomRowKind::Invite)
    }
}

pub fn invite_avatar_path(invite: &Invite, media: &dyn MediaCache) -> Option<PathBuf> {
    room_or_space_avatar(invite.avatar_mxc.as_deref(), invite.is_space(), media)
}

fn room_or_space_avatar(
    mxc: Option<&str>,
    is_space: bool,
    media: &dyn MediaCache,
) -> Option<PathBuf> {
    let mxc = mxc?;
    if is_space {
        media.space_avatar_path(mxc)
    } else {
        media.room_avatar_path(mxc)
    }
}

pub fn removal_to_dto(removal: &Removal, media: &dyn MediaCache) -> RoomDto {
    let avatar = removal_avatar_path(removal, media).and_then(|path| peek_avatar(&path));
    RoomDto {
        name: SharedString::from(&removal.name),
        initial: SharedString::from(avatar_initials(&removal.name)),
        color_index: avatar_color_index(removal.id.as_ref()),
        last_message_time: SharedString::from(&room_activity_label(removal.removed_ts)),
        invite_kind: removal_shape(removal),
        removal: removal.kind,
        removed_by: SharedString::from(removal.by.shown()),
        has_avatar: avatar.is_some(),
        avatar,
        ..bare_row_dto(removal.id.as_ref(), RoomRowKind::Removal)
    }
}

pub fn removal_shape(removal: &Removal) -> InviteKind {
    InviteKind::of(removal.is_space, removal.is_direct)
}

pub fn removal_avatar_path(removal: &Removal, media: &dyn MediaCache) -> Option<PathBuf> {
    room_or_space_avatar(removal.avatar_mxc.as_deref(), removal.is_space, media)
}

pub fn record_removal_avatar_need(removal: &Removal, media: &dyn MediaCache) -> ShownAvatar {
    let avatar_path = removal_avatar_path(removal, media);
    record_avatar_need(
        &AvatarSlot::Room(removal.id.as_ref().to_owned()),
        avatar_path.as_deref(),
    )
}

pub fn room_preview_avatar_path(room: &RoomPreview, media: &dyn MediaCache) -> Option<PathBuf> {
    room_or_space_avatar(room.avatar_mxc.as_deref(), room.is_space, media)
}

pub fn record_invite_avatar_need(invite: &Invite, media: &dyn MediaCache) -> ShownAvatar {
    let avatar_path = invite_avatar_path(invite, media);
    record_avatar_need(
        &AvatarSlot::Room(invite.id.as_ref().to_owned()),
        avatar_path.as_deref(),
    )
}

pub fn space_match_to_dto(m: &SpaceMatch, media: &dyn MediaCache) -> RoomDto {
    let avatar = space_match_avatar_path(m, media).and_then(|path| peek_avatar(&path));
    RoomDto {
        name: SharedString::from(&m.name),
        initial: SharedString::from(avatar_initials(&m.name)),
        color_index: avatar_color_index(&m.id),
        alert: m.flags.alert,
        mention: m.flags.mention,
        hint: m.flags.hint,
        space_rooms: saturating_i32(m.rooms),
        space_parent: SharedString::from(m.parent.as_deref().unwrap_or_default()),
        has_avatar: avatar.is_some(),
        avatar,
        ..bare_row_dto(&m.id, RoomRowKind::Space)
    }
}

fn bare_row_dto(id: &str, kind: RoomRowKind) -> RoomDto {
    RoomDto {
        id: SharedString::from(id),
        kind,
        name: SharedString::new(),
        initial: SharedString::new(),
        color_index: 0,
        members: 0,
        alert: false,
        mention: false,
        hint: false,
        muted: false,
        last_message_sender: SharedString::new(),
        last_message_kind: MessagePreviewKind::None,
        last_message_body: SharedString::new(),
        last_message_service_kind: ServiceKind::None,
        last_message_service_target: SharedString::new(),
        last_message_is_own: false,
        last_message_edited: false,
        last_message_time: SharedString::new(),
        space_rooms: 0,
        space_parent: SharedString::new(),
        invite_count: 0,
        invite_kind: InviteKind::Room,
        inviter: SharedString::new(),
        inviter_id: SharedString::new(),
        removal: RemovalKind::Kicked,
        removed_by: SharedString::new(),
        call: RoomCallMark::None,
        avatar: None,
        has_avatar: false,
    }
}

pub fn request_space_match_avatar(m: &SpaceMatch, media: &dyn MediaCache) -> ShownAvatar {
    let path = space_match_avatar_path(m, media);
    let slot = AvatarSlot::Space(m.id.clone());
    let shown = record_avatar_need(&slot, path.as_deref());
    load_avatar_async(path.as_deref(), slot);
    shown
}

fn space_match_avatar_path(m: &SpaceMatch, media: &dyn MediaCache) -> Option<PathBuf> {
    media.space_avatar_path(m.avatar_mxc.as_deref()?)
}

pub fn space_to_dto(s: &Space, media: &dyn MediaCache) -> SpaceDto {
    let avatar = s
        .avatar_mxc
        .as_deref()
        .and_then(|mxc| media.space_avatar_path(mxc))
        .and_then(|path| peek_avatar(&path));
    SpaceDto {
        id: SharedString::from(&s.id),
        name: SharedString::from(&s.name),
        alert: s.alert,
        mention: s.mention,
        hint: s.hint,
        initial: SharedString::from(sender_initial(&s.name)),
        has_avatar: avatar.is_some(),
        avatar,
    }
}

pub fn prefetch_space_avatar(s: &Space, media: &dyn MediaCache) {
    let avatar_path = s
        .avatar_mxc
        .as_deref()
        .and_then(|mxc| media.space_avatar_path(mxc));
    load_avatar_async(avatar_path.as_deref(), AvatarSlot::Space(s.id.clone()));
}

pub fn space_child_to_dto(row: &SpaceIndexRow, media: &dyn MediaCache) -> SpaceChildDto {
    let child = &row.child;
    let (is_space, children) = match child.kind {
        ChildKind::Room => (false, 0),
        ChildKind::Space { children } => (true, saturating_i32(children)),
    };
    let avatar = space_child_avatar_path(child, media).and_then(|path| peek_avatar(&path));
    SpaceChildDto {
        id: SharedString::from(child.id.as_ref()),
        name: SharedString::from(&child.name),
        initial: SharedString::from(avatar_initials(&child.name)),
        color_index: avatar_color_index(child.id.as_ref()),
        detail: SharedString::from(space_child_detail(child)),
        members: saturating_i32(child.member_count),
        is_space,
        children,
        access: row.access,
        has_avatar: avatar.is_some(),
        avatar,
    }
}

pub fn request_space_child_avatar(row: &SpaceIndexRow, media: &dyn MediaCache) {
    let path = space_child_avatar_path(&row.child, media);
    load_avatar_async(
        path.as_deref(),
        AvatarSlot::SpaceChild(row.child.id.to_string()),
    );
}

pub fn log_line_to_dto(line: &LogLine) -> LogLineDto {
    LogLineDto {
        time: SharedString::from(log_time_label(line.at)),
        level: line.level,
        level_name: SharedString::from(log_level_name(line.level)),
        target: SharedString::from(line.target),
        text: SharedString::from(line.text.as_str()),
    }
}

pub fn load_room_info_avatar(card: &RoomCard, media: &dyn MediaCache) -> Option<Image> {
    let path = card
        .avatar_mxc
        .as_deref()
        .and_then(|mxc| media.room_avatar_path(mxc));
    load_avatar_async(path.as_deref(), AvatarSlot::RoomInfo)
}

fn space_child_avatar_path(child: &SpaceChild, media: &dyn MediaCache) -> Option<PathBuf> {
    let is_space = match child.kind {
        ChildKind::Room => false,
        ChildKind::Space { .. } => true,
    };
    room_or_space_avatar(child.avatar_mxc.as_deref(), is_space, media)
}

fn space_child_detail(child: &SpaceChild) -> &str {
    child
        .topic
        .as_deref()
        .and_then(|topic| topic.lines().find(|line| !line.trim().is_empty()))
        .or(child.alias.as_deref())
        .unwrap_or_default()
}
