use serde::Serialize;

use super::calls::{RoomCallDto, room_call};
use super::{MessageDto, message};
use crate::adapters::demo::probe::names;
use crate::commands::messages::UserMessageKind;
use crate::commands::view::{
    CreateRoomView, DirectoryView, InvitePeopleView, InviteView, InviteeRow, InvitesSection,
    LeaveSpaceView, RemovalView, RoomInfoView, RoomLogView, RoomMenuTarget, RoomPreviewSheet,
    RoomPreviewView, RosterRow, SpaceIndexRow, SpaceIndexView, SpaceMatch, UpgradeView,
};
use crate::domain::departure::{AdminStanding, Removal};
use crate::domain::invite::Invite;
use crate::domain::member_name::NamedUser;
use crate::domain::room::{Room, Space};
use crate::domain::room_info::RosterSection;
use crate::domain::room_preview::{PreviewMembership, RoomPreview};
use crate::domain::room_state::HistoryVisibility;
use crate::domain::space_index::{ChildKind, JoinRule};

#[derive(Serialize)]
pub(super) struct CreateRoomDto {
    status: &'static str,
    kind: &'static str,
    parent: Option<String>,
    server: String,
    error: MessageDto,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub(super) struct InvitePeopleDto {
    open: bool,
    room_id: Option<String>,
    room_name: Option<String>,
    is_space: bool,
    search: &'static str,
    limited: bool,
    sending: bool,
    error: MessageDto,
    avatars_ready: usize,
    results: Vec<InviteeDto>,
    picked: Vec<InviteeDto>,
}

#[derive(Serialize)]
struct InviteeDto {
    user_id: String,
    shown: String,
    has_avatar_mxc: bool,
    membership: &'static str,
    state: &'static str,
    failure: Option<MessageDto>,
}

#[derive(Serialize)]
pub(super) struct UpgradeDto {
    room_id: Option<String>,
    successor: &'static str,
    successor_id: Option<String>,
    predecessor: &'static str,
    predecessor_id: Option<String>,
}

#[derive(Serialize)]
pub(super) struct RoomLogDto {
    room_id: String,
    name: String,
    lines: usize,
    dropped: u64,
    lines_landed: i32,
    newest: Vec<String>,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub(super) struct RoomMenuDto {
    room_id: String,
    name: String,
    is_space: bool,
    unread: bool,
    notify: &'static str,
    notify_busy: bool,
    leaving: bool,
    may_invite: bool,
    may_add_children: bool,
    has_call: bool,
}

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
pub(super) struct RoomPreviewDto {
    status: &'static str,
    address: Option<String>,
    entry: bool,
    access: &'static str,
    room: Option<PreviewedRoomDto>,
    opens_event: bool,
    error: MessageDto,
    avatars_ready: usize,
}

#[derive(Serialize)]
struct PreviewedRoomDto {
    room_id: String,
    name: String,
    alias: Option<String>,
    topic: Option<String>,
    members: u64,
    is_space: bool,
    join_rule: Option<&'static str>,
    membership: &'static str,
    has_avatar_mxc: bool,
}

#[derive(Serialize)]
pub(super) struct InviteSheetDto {
    open: bool,
    invite: Option<InviteDto>,
    answer: &'static str,
    error: MessageDto,
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
pub(super) struct RemovalSheetDto {
    open: bool,
    removal: Option<RemovalDto>,
    forgetting: bool,
    error: MessageDto,
}

#[derive(Serialize)]
pub(super) struct LeaveSpaceDto {
    open: bool,
    space_id: Option<String>,
    name: Option<String>,
    rooms: usize,
    spaces: usize,
    standing: Option<&'static str>,
    leaving: bool,
    error: MessageDto,
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

#[derive(Serialize)]
pub(super) struct SpaceIndexDto {
    status: &'static str,
    space_name: String,
    avatars_ready: usize,
    pages_landed: i32,
    rows: Vec<SpaceChildDto>,
}

#[derive(Serialize)]
struct SpaceChildDto {
    id: String,
    name: String,
    kind: &'static str,
    children: Option<u64>,
    members: u64,
    join_rule: &'static str,
    via: Vec<String>,
    has_avatar_mxc: bool,
    access: &'static str,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub(super) struct RoomInfoDto {
    open: bool,
    placement: &'static str,
    room_id: Option<String>,
    name: Option<String>,
    member_count: Option<u64>,
    is_direct: bool,
    topic: Option<String>,
    topic_html: Option<String>,
    alias: Option<String>,
    joined_at: Option<u64>,
    link: Option<String>,
    notify: &'static str,
    notify_busy: bool,
    leaving: bool,
    may_invite: bool,
    readable_by_anyone: bool,
    roster: &'static str,
    has_more: bool,
    pages_landed: i32,
    avatars_ready: usize,
    error: MessageDto,
    rows: Vec<MemberRowDto>,
}

#[derive(Serialize)]
struct MemberRowDto {
    kind: &'static str,
    user_id: Option<String>,
    name: Option<String>,
    role: Option<&'static str>,
    invited: bool,
    has_avatar_mxc: bool,
}

pub(super) fn create_room(source: &CreateRoomView) -> CreateRoomDto {
    CreateRoomDto {
        status: names::create_room_status(source.status),
        kind: names::room_kind(source.kind),
        parent: (!source.parent_name.is_empty()).then(|| source.parent_name.clone()),
        server: source.server_name.clone(),
        error: message(&source.error),
    }
}

pub(super) fn invite_people(source: &InvitePeopleView) -> InvitePeopleDto {
    let target = source.target.as_ref();
    InvitePeopleDto {
        open: target.is_some(),
        room_id: target.map(|target| target.room_id.to_string()),
        room_name: target.map(|target| target.name.clone()),
        is_space: target.is_some_and(|target| target.space),
        search: names::people_search_status(source.search),
        limited: source.limited,
        sending: source.sending,
        error: message(&source.error),
        avatars_ready: source.avatars_ready,
        results: source.results.iter().map(invitee).collect(),
        picked: source.picked.iter().map(invitee).collect(),
    }
}

fn invitee(source: &InviteeRow) -> InviteeDto {
    InviteeDto {
        user_id: source.person.user_id.to_string(),
        shown: source.person.shown(),
        has_avatar_mxc: source.person.avatar_mxc.is_some(),
        membership: names::room_membership(source.person.membership),
        state: names::invitee_state(source.state),
        failure: (source.failure.kind != UserMessageKind::None).then(|| message(&source.failure)),
    }
}

pub(super) fn upgrade(source: &UpgradeView) -> UpgradeDto {
    UpgradeDto {
        room_id: source.room_id.as_ref().map(ToString::to_string),
        successor: names::upgrade_link(&source.successor),
        successor_id: source.successor.room_id().map(ToString::to_string),
        predecessor: names::upgrade_link(&source.predecessor),
        predecessor_id: source.predecessor.room_id().map(ToString::to_string),
    }
}

const NEWEST_LOG_LINES: usize = 12;

pub(super) fn room_log(source: &RoomLogView) -> RoomLogDto {
    let lines = &source.log.lines;
    RoomLogDto {
        room_id: source.room_id.to_string(),
        name: source.name.clone(),
        lines: lines.len(),
        dropped: source.log.dropped,
        lines_landed: source.lines_landed,
        newest: lines
            .iter()
            .skip(lines.len().saturating_sub(NEWEST_LOG_LINES))
            .map(|line| {
                format!(
                    "{} {}: {}",
                    names::log_level(line.level),
                    line.target,
                    line.text
                )
            })
            .collect(),
    }
}

pub(super) fn room_menu(source: &RoomMenuTarget) -> RoomMenuDto {
    RoomMenuDto {
        room_id: source.room_id.to_string(),
        name: source.name.clone(),
        is_space: source.is_space,
        unread: source.unread,
        notify: names::notify_mode(source.notify),
        notify_busy: source.notify_busy,
        leaving: source.leaving,
        may_invite: source.may_invite,
        may_add_children: source.may_add_children,
        has_call: source.has_call,
    }
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

pub(super) fn removal_sheet(source: &RemovalView) -> RemovalSheetDto {
    RemovalSheetDto {
        open: source.removal.is_some(),
        removal: source.removal.as_deref().map(removal),
        forgetting: source.forgetting,
        error: message(&source.error),
    }
}

pub(super) fn leave_space(source: &LeaveSpaceView) -> LeaveSpaceDto {
    let space = source.space.as_ref();
    LeaveSpaceDto {
        open: space.is_some(),
        space_id: space.map(|space| space.id.to_string()),
        name: space.map(|space| space.name.clone()),
        rooms: space.map_or(0, |space| space.rooms),
        spaces: space.map_or(0, |space| space.spaces),
        standing: source.standing.map(|standing| match standing {
            AdminStanding::OnlyAdmin => "only-admin",
            AdminStanding::NotOnlyAdmin => "not-only-admin",
        }),
        leaving: source.leaving,
        error: message(&source.error),
    }
}

pub(super) fn invite_sheet(source: &InviteView) -> InviteSheetDto {
    InviteSheetDto {
        open: source.invite.is_some(),
        invite: source.invite.as_deref().map(invite),
        answer: names::pending_answer(source.answer),
        error: message(&source.error),
    }
}

pub(super) fn room_preview(source: &RoomPreviewView) -> RoomPreviewDto {
    let shown = match &source.sheet {
        RoomPreviewSheet::Loaded(shown) => Some(shown),
        RoomPreviewSheet::Closed
        | RoomPreviewSheet::Idle
        | RoomPreviewSheet::Loading
        | RoomPreviewSheet::Missing
        | RoomPreviewSheet::Unavailable
        | RoomPreviewSheet::Failed => None,
    };
    RoomPreviewDto {
        status: names::room_preview_status(&source.sheet),
        address: (!source.address.is_empty()).then(|| source.address.clone()),
        entry: source.entry,
        access: names::child_access(source.access),
        room: shown.map(|shown| previewed_room(&shown.room)),
        opens_event: shown.is_some_and(|shown| shown.opens_event),
        error: message(&source.error),
        avatars_ready: source.avatars_ready,
    }
}

fn previewed_room(source: &RoomPreview) -> PreviewedRoomDto {
    PreviewedRoomDto {
        room_id: source.id.to_string(),
        name: source.name.clone(),
        alias: source.alias.clone(),
        topic: source.topic.clone(),
        members: source.member_count,
        is_space: source.is_space,
        join_rule: source.join_rule.as_ref().map(join_rule),
        membership: match source.membership {
            PreviewMembership::Outsider => "outsider",
            PreviewMembership::Joined => "joined",
            PreviewMembership::Invited => "invited",
            PreviewMembership::Knocked => "knocked",
            PreviewMembership::Banned => "banned",
        },
        has_avatar_mxc: source.avatar_mxc.is_some(),
    }
}

pub(super) fn space_index(source: &SpaceIndexView) -> SpaceIndexDto {
    SpaceIndexDto {
        status: names::space_index_status(source.status),
        space_name: source.space_name.clone(),
        avatars_ready: source.avatars_ready,
        pages_landed: source.pages_landed,
        rows: source.rows.iter().map(space_child).collect(),
    }
}

pub(super) fn room_info(source: &RoomInfoView) -> RoomInfoDto {
    let card = source.card.as_ref();
    let about = source.about.as_ref();
    RoomInfoDto {
        open: card.is_some(),
        placement: names::room_info_placement(source.placement),
        room_id: card.map(|card| card.id.to_string()),
        name: card.map(|card| card.name.clone()),
        member_count: card.map(|card| card.member_count),
        is_direct: card.is_some_and(|card| card.is_direct),
        topic: card.and_then(|card| card.topic.clone()),
        topic_html: about
            .and_then(|about| about.topic.as_ref())
            .and_then(|topic| topic.html().map(str::to_owned)),
        alias: card.and_then(|card| card.alias.clone()),
        joined_at: about.and_then(|about| about.joined_at),
        link: about.map(|about| about.link.clone()),
        notify: names::notify_mode(source.notify),
        notify_busy: source.notify_busy,
        leaving: source.leaving,
        may_invite: card.is_some_and(|card| card.may_invite),
        readable_by_anyone: card.is_some_and(|card| card.readable_by_anyone),
        roster: names::roster_status(source.roster),
        has_more: source.has_more,
        pages_landed: source.pages_landed,
        avatars_ready: source.avatars_ready,
        error: message(&source.error),
        rows: source.rows.iter().map(member_row).collect(),
    }
}

fn member_row(source: &RosterRow) -> MemberRowDto {
    match source {
        RosterRow::InvitedHeading => MemberRowDto {
            kind: "invited-heading",
            user_id: None,
            name: None,
            role: None,
            invited: true,
            has_avatar_mxc: false,
        },
        RosterRow::Member(member) => MemberRowDto {
            kind: "member",
            user_id: Some(member.user_id.clone()),
            name: Some(member.name.shown(&member.user_id)),
            role: Some(names::member_role(member.role)),
            invited: member.section == RosterSection::Invited,
            has_avatar_mxc: member.avatar_mxc.is_some(),
        },
    }
}

fn space_child(source: &SpaceIndexRow) -> SpaceChildDto {
    let child = &source.child;
    let (kind, children) = match child.kind {
        ChildKind::Room => ("room", None),
        ChildKind::Space { children } => ("space", Some(children)),
    };
    SpaceChildDto {
        id: child.id.to_string(),
        name: child.name.clone(),
        kind,
        children,
        members: child.member_count,
        join_rule: join_rule(&child.join_rule),
        via: child.via.clone(),
        has_avatar_mxc: child.avatar_mxc.is_some(),
        access: names::child_access(source.access),
    }
}

fn join_rule(source: &JoinRule) -> &'static str {
    match source {
        JoinRule::Public => "public",
        JoinRule::Restricted { .. } => "restricted",
        JoinRule::KnockRestricted { .. } => "knock_restricted",
        JoinRule::Knock => "knock",
        JoinRule::Invite => "invite",
        JoinRule::Private => "private",
        JoinRule::Unsupported => "unsupported",
    }
}

fn history_visibility(source: HistoryVisibility) -> &'static str {
    match source {
        HistoryVisibility::WorldReadable => "world_readable",
        HistoryVisibility::Shared => "shared",
        HistoryVisibility::Invited => "invited",
        HistoryVisibility::Joined => "joined",
        HistoryVisibility::Unknown => "unknown",
    }
}
