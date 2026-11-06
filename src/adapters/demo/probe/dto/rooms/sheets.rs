use serde::Serialize;

use super::{InviteDto, RemovalDto, invite, removal};
use crate::adapters::demo::probe::dto::{MessageDto, message};
use crate::adapters::demo::probe::names;
use crate::commands::messages::UserMessageKind;
use crate::commands::view::{
    CreateRoomView, InvitePeopleView, InviteView, InviteeRow, LeaveSpaceView, RemovalView,
    RoomInfoView, RoomLogView, RoomMenuTarget, RoomPreviewSheet, RoomPreviewView, RosterRow,
    SpaceIndexRow, SpaceIndexView, UpgradeView,
};
use crate::domain::departure::AdminStanding;
use crate::domain::room_info::RosterSection;
use crate::domain::room_preview::{PreviewMembership, RoomPreview};
use crate::domain::room_state::HistoryVisibility;
use crate::domain::space_index::{ChildKind, JoinRule};

#[derive(Serialize)]
pub(in crate::adapters::demo::probe::dto) struct CreateRoomDto {
    status: &'static str,
    kind: &'static str,
    parent: Option<String>,
    server: String,
    error: MessageDto,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub(in crate::adapters::demo::probe::dto) struct InvitePeopleDto {
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
pub(in crate::adapters::demo::probe::dto) struct UpgradeDto {
    room_id: Option<String>,
    successor: &'static str,
    successor_id: Option<String>,
    predecessor: &'static str,
    predecessor_id: Option<String>,
}

#[derive(Serialize)]
pub(in crate::adapters::demo::probe::dto) struct RoomLogDto {
    room_id: String,
    name: String,
    lines: usize,
    dropped: u64,
    lines_landed: i32,
    newest: Vec<String>,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub(in crate::adapters::demo::probe::dto) struct RoomMenuDto {
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
pub(in crate::adapters::demo::probe::dto) struct RoomPreviewDto {
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
pub(in crate::adapters::demo::probe::dto) struct InviteSheetDto {
    open: bool,
    invite: Option<InviteDto>,
    answer: &'static str,
    error: MessageDto,
}

#[derive(Serialize)]
pub(in crate::adapters::demo::probe::dto) struct RemovalSheetDto {
    open: bool,
    removal: Option<RemovalDto>,
    forgetting: bool,
    error: MessageDto,
}

#[derive(Serialize)]
pub(in crate::adapters::demo::probe::dto) struct LeaveSpaceDto {
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
pub(in crate::adapters::demo::probe::dto) struct SpaceIndexDto {
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
pub(in crate::adapters::demo::probe::dto) struct RoomInfoDto {
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

pub(in crate::adapters::demo::probe::dto) fn create_room(source: &CreateRoomView) -> CreateRoomDto {
    CreateRoomDto {
        status: names::create_room_status(source.status),
        kind: names::room_kind(source.kind),
        parent: (!source.parent_name.is_empty()).then(|| source.parent_name.clone()),
        server: source.server_name.clone(),
        error: message(&source.error),
    }
}

pub(in crate::adapters::demo::probe::dto) fn invite_people(
    source: &InvitePeopleView,
) -> InvitePeopleDto {
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

pub(in crate::adapters::demo::probe::dto) fn upgrade(source: &UpgradeView) -> UpgradeDto {
    UpgradeDto {
        room_id: source.room_id.as_ref().map(ToString::to_string),
        successor: names::upgrade_link(&source.successor),
        successor_id: source.successor.room_id().map(ToString::to_string),
        predecessor: names::upgrade_link(&source.predecessor),
        predecessor_id: source.predecessor.room_id().map(ToString::to_string),
    }
}

const NEWEST_LOG_LINES: usize = 12;

pub(in crate::adapters::demo::probe::dto) fn room_log(source: &RoomLogView) -> RoomLogDto {
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

pub(in crate::adapters::demo::probe::dto) fn room_menu(source: &RoomMenuTarget) -> RoomMenuDto {
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

pub(in crate::adapters::demo::probe::dto) fn removal_sheet(
    source: &RemovalView,
) -> RemovalSheetDto {
    RemovalSheetDto {
        open: source.removal.is_some(),
        removal: source.removal.as_deref().map(removal),
        forgetting: source.forgetting,
        error: message(&source.error),
    }
}

pub(in crate::adapters::demo::probe::dto) fn leave_space(source: &LeaveSpaceView) -> LeaveSpaceDto {
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

pub(in crate::adapters::demo::probe::dto) fn invite_sheet(source: &InviteView) -> InviteSheetDto {
    InviteSheetDto {
        open: source.invite.is_some(),
        invite: source.invite.as_deref().map(invite),
        answer: names::pending_answer(source.answer),
        error: message(&source.error),
    }
}

pub(in crate::adapters::demo::probe::dto) fn room_preview(
    source: &RoomPreviewView,
) -> RoomPreviewDto {
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

pub(in crate::adapters::demo::probe::dto) fn space_index(source: &SpaceIndexView) -> SpaceIndexDto {
    SpaceIndexDto {
        status: names::space_index_status(source.status),
        space_name: source.space_name.clone(),
        avatars_ready: source.avatars_ready,
        pages_landed: source.pages_landed,
        rows: source.rows.iter().map(space_child).collect(),
    }
}

pub(in crate::adapters::demo::probe::dto) fn room_info(source: &RoomInfoView) -> RoomInfoDto {
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

pub(super) fn history_visibility(source: HistoryVisibility) -> &'static str {
    match source {
        HistoryVisibility::WorldReadable => "world_readable",
        HistoryVisibility::Shared => "shared",
        HistoryVisibility::Invited => "invited",
        HistoryVisibility::Joined => "joined",
        HistoryVisibility::Unknown => "unknown",
    }
}
