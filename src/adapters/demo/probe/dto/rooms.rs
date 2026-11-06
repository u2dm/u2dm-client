mod sheets;

use serde::Serialize;

use self::sheets::history_visibility;
pub(super) use self::sheets::{
    CreateRoomDto, InvitePeopleDto, InviteSheetDto, LeaveSpaceDto, RemovalSheetDto, RoomInfoDto,
    RoomLogDto, RoomMenuDto, RoomPreviewDto, SpaceIndexDto, UpgradeDto, create_room, invite_people,
    invite_sheet, leave_space, removal_sheet, room_info, room_log, room_menu, room_preview,
    space_index, upgrade,
};
use super::calls::{RoomCallDto, room_call};
use crate::adapters::demo::probe::names;
use crate::commands::view::{DirectoryView, InvitesSection, SpaceMatch};
use crate::domain::departure::Removal;
use crate::domain::invite::Invite;
use crate::domain::member_name::NamedUser;
use crate::domain::room::{Room, Space};

#[derive(Serialize)]
struct PollPermissionsDto {
    vote: bool,
    end: bool,
    start: bool,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct MessagePermissionsDto {
    send_messages: bool,
    react: bool,
    send_stickers: bool,
    delete_own: bool,
    delete_others: bool,
    pin: bool,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct RoomDto {
    id: String,
    name: String,
    is_direct: bool,
    direct_partner: Option<String>,
    is_encrypted: bool,
    history_visibility: &'static str,
    poll_permissions: PollPermissionsDto,
    message_permissions: MessagePermissionsDto,
    may_invite: bool,
    may_join_calls: bool,
    call: Option<RoomCallDto>,
    member_count: u64,
    has_unread: bool,
    has_mentions: bool,
    has_activity: bool,
    muted: bool,
    alert: bool,
    mention: bool,
    hint: bool,
    last_activity_ts: u64,
    last_message_sender: Option<String>,
    last_message_body: String,
    last_message_is_own: bool,
    last_message_edited: bool,
    successor: Option<String>,
    predecessor: Option<String>,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct SpaceDto {
    id: String,
    name: String,
    member_count: u64,
    child_room_ids: Vec<String>,
    child_space_ids: Vec<String>,
    order: Option<String>,
    may_invite: bool,
    may_add_children: bool,
    alert: bool,
    mention: bool,
    hint: bool,
}

#[derive(Serialize)]
struct SpaceHeadingDto {
    id: String,
    name: String,
    member_count: u64,
    may_invite: bool,
    may_add_children: bool,
}

#[derive(Serialize)]
struct SpaceMatchDto {
    id: String,
    name: String,
    parent: Option<String>,
    rooms: usize,
    alert: bool,
    mention: bool,
    hint: bool,
}

#[derive(Serialize)]
struct InviteDto {
    id: String,
    name: String,
    kind: &'static str,
    inviter: Option<String>,
    inviter_name: Option<String>,
    topic: Option<String>,
    has_avatar_mxc: bool,
    received_ts: u64,
}

#[derive(Serialize)]
struct RemovalDto {
    id: String,
    name: String,
    kind: &'static str,
    is_space: bool,
    is_direct: bool,
    by: String,
    by_name: String,
    reason: Option<String>,
    has_avatar_mxc: bool,
    removed_ts: u64,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub(super) struct DirectoryDto {
    scope: &'static str,
    space_id: String,
    space_name: String,
    subspace_id: String,
    listed_space: SpaceHeadingDto,
    direct_alert: bool,
    direct_mention: bool,
    direct_hint: bool,
    invites_pending: bool,
    attention: usize,
    invites: Vec<InviteDto>,
    invites_section: &'static str,
    removals: Vec<RemovalDto>,
    rooms: Vec<RoomDto>,
    space_matches: Vec<SpaceMatchDto>,
    spaces: Vec<SpaceDto>,
    subspaces: Vec<SpaceDto>,
}

fn room(source: &Room) -> RoomDto {
    RoomDto {
        id: source.id.to_string(),
        name: source.display_name.clone(),
        is_direct: source.is_direct,
        direct_partner: source.direct_partner.as_ref().map(ToString::to_string),
        is_encrypted: source.is_encrypted,
        history_visibility: history_visibility(source.history_visibility),
        poll_permissions: PollPermissionsDto {
            vote: source.poll_permissions.vote,
            end: source.poll_permissions.end,
            start: source.poll_permissions.start,
        },
        message_permissions: MessagePermissionsDto {
            send_messages: source.message_permissions.send_messages,
            react: source.message_permissions.react,
            send_stickers: source.message_permissions.send_stickers,
            delete_own: source.message_permissions.delete_own,
            delete_others: source.message_permissions.delete_others,
            pin: source.message_permissions.pin,
        },
        may_invite: source.may_invite,
        may_join_calls: source.may_join_calls,
        call: source.call.as_deref().map(room_call),
        member_count: source.member_count,
        has_unread: source.has_unread,
        has_mentions: source.has_mentions,
        has_activity: source.has_activity,
        muted: source.muted(),
        alert: source.alert(),
        mention: source.mention(),
        hint: source.hint(),
        last_activity_ts: source.last_activity_ts,
        last_message_sender: source.last_message_sender.as_ref().map(NamedUser::shown),
        last_message_body: source.last_message_body.plain.clone(),
        last_message_is_own: source.last_message_is_own,
        last_message_edited: source.last_message_edited,
        successor: source.successor.as_ref().map(ToString::to_string),
        predecessor: source.predecessor.as_ref().map(ToString::to_string),
    }
}

fn space(source: &Space) -> SpaceDto {
    SpaceDto {
        id: source.id.clone(),
        name: source.name.clone(),
        member_count: source.member_count,
        child_room_ids: source.child_room_ids.clone(),
        child_space_ids: source.child_space_ids.clone(),
        order: source.order.clone(),
        may_invite: source.powers.invite,
        may_add_children: source.powers.add_children,
        alert: source.alert,
        mention: source.mention,
        hint: source.hint,
    }
}

fn space_match(source: &SpaceMatch) -> SpaceMatchDto {
    SpaceMatchDto {
        id: source.id.clone(),
        name: source.name.clone(),
        parent: source.parent.clone(),
        rooms: source.rooms,
        alert: source.flags.alert,
        mention: source.flags.mention,
        hint: source.flags.hint,
    }
}

pub(super) fn directory(source: &DirectoryView) -> DirectoryDto {
    DirectoryDto {
        scope: names::room_scope(source.scope),
        space_id: source.space_id.clone(),
        space_name: source.space_name.clone(),
        subspace_id: source.subspace_id.clone(),
        listed_space: SpaceHeadingDto {
            id: source.listed_space.id.clone(),
            name: source.listed_space.name.clone(),
            member_count: source.listed_space.member_count,
            may_invite: source.listed_space.may_invite,
            may_add_children: source.listed_space.may_add_children,
        },
        direct_alert: source.direct_flags.alert,
        direct_mention: source.direct_flags.mention,
        direct_hint: source.direct_flags.hint,
        invites_pending: source.invites_pending,
        attention: source.attention,
        invites: source.invites.iter().map(|entry| invite(entry)).collect(),
        invites_section: invites_section_word(source.invites_section),
        removals: source.removals.iter().map(|entry| removal(entry)).collect(),
        rooms: source.rooms.iter().map(|entry| room(entry)).collect(),
        space_matches: source.space_matches.iter().map(space_match).collect(),
        spaces: source.spaces.iter().map(space).collect(),
        subspaces: source.subspaces.iter().map(space).collect(),
    }
}

const fn invites_section_word(section: InvitesSection) -> &'static str {
    match section {
        InvitesSection::Folded => "folded",
        InvitesSection::Unfolded => "unfolded",
        InvitesSection::Matches => "matches",
    }
}

fn invite(source: &Invite) -> InviteDto {
    InviteDto {
        id: source.id.to_string(),
        name: source.name.clone(),
        kind: names::invite_kind(source.kind),
        inviter: source
            .inviter
            .as_ref()
            .map(|inviter| inviter.user_id.clone()),
        inviter_name: source.inviter.as_ref().map(NamedUser::shown),
        topic: source.topic.clone(),
        has_avatar_mxc: source.avatar_mxc.is_some(),
        received_ts: source.received_ts,
    }
}

fn removal(source: &Removal) -> RemovalDto {
    RemovalDto {
        id: source.id.to_string(),
        name: source.name.clone(),
        kind: names::removal_kind(source.kind),
        is_space: source.is_space,
        is_direct: source.is_direct,
        by: source.by.user_id.clone(),
        by_name: source.by.shown(),
        reason: source.reason.clone(),
        has_avatar_mxc: source.avatar_mxc.is_some(),
        removed_ts: source.removed_ts,
    }
}
