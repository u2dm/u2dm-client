use std::collections::HashSet;
use std::mem;

use super::data;
use super::lists::{RoomOverrides, created_space};
use super::people::room_members;
use crate::adapters::demo::dto::{DemoData, InviteDto, RoomDto, SpaceDto, UnjoinedDto};
use crate::domain::room::{RoomId, Space};
use crate::domain::room_info::RoomAbout;
use crate::domain::room_preview::{PreviewMembership, RoomPreview};
use crate::domain::space_index::{ChildKind, SpaceChild};
use crate::util::unix_now_ms;

pub(super) const DEMO_VIA: &str = "demo.local";

pub(super) const COPY_SUFFIX: &str = "-copy";

pub(super) const MATRIX_TO: &str = "https://matrix.to/#/";

pub fn room_about(room_id: &RoomId) -> Option<RoomAbout> {
    let data = data();
    if let Some(room) = data.rooms.iter().find(|room| room.id == room_id.as_ref()) {
        return Some(RoomAbout {
            joined_at: room.joined_at(unix_now_ms()),
            link: room_link(room_id, room.alias.as_deref()),
            topic: room.rich_topic(),
        });
    }
    if let Some(entry) = data
        .invites
        .iter()
        .find(|entry| entry.id == room_id.as_ref())
    {
        return Some(RoomAbout {
            joined_at: None,
            link: room_link(room_id, None),
            topic: entry.rich_topic(),
        });
    }
    let entry = data
        .unjoined
        .iter()
        .find(|entry| entry.id == room_id.as_ref())?;
    Some(RoomAbout {
        joined_at: None,
        link: room_link(room_id, entry.alias()),
        topic: entry.rich_topic(),
    })
}

pub fn event_link(room_id: &RoomId, event_id: &str) -> String {
    format!("{MATRIX_TO}{room_id}/{event_id}?via={DEMO_VIA}")
}

pub fn room_link_of(room_id: &RoomId) -> Option<String> {
    room_about(room_id).map(|about| about.link).or_else(|| {
        data()
            .spaces
            .iter()
            .any(|space| space.id() == room_id.as_ref())
            .then(|| room_link(room_id, None))
    })
}

pub(super) fn room_link(room_id: &RoomId, alias: Option<&str>) -> String {
    match alias.and_then(|alias| alias.strip_prefix('#')) {
        Some(alias) => format!("{MATRIX_TO}%23{alias}"),
        None => format!("{MATRIX_TO}{room_id}?via={DEMO_VIA}"),
    }
}

pub fn spaces_with(joined: &[RoomId], overrides: &RoomOverrides) -> Vec<Space> {
    let data = data();
    let mut spaces: Vec<Space> = data
        .spaces
        .iter()
        .map(SpaceDto::to_space)
        .chain(
            data.unjoined
                .iter()
                .filter(|entry| entry.space && was_joined(joined, &entry.id))
                .map(UnjoinedDto::to_space),
        )
        .chain(
            data.invites
                .iter()
                .filter(|entry| entry.space && was_joined(joined, &entry.id))
                .map(InviteDto::to_space),
        )
        .chain(
            overrides
                .created
                .iter()
                .filter(|created| created.space)
                .map(created_space),
        )
        .filter(|space| !overrides.left.contains(&space.id))
        .collect();
    for space in &mut spaces {
        if let Some(children) = overrides.created_children.get(&space.id) {
            space.child_room_ids.extend(children.iter().cloned());
        }
    }
    let joined_spaces: HashSet<String> = spaces.iter().map(|space| space.id.clone()).collect();
    for space in &mut spaces {
        (space.child_space_ids, space.child_room_ids) = mem::take(&mut space.child_room_ids)
            .into_iter()
            .chain(mem::take(&mut space.child_space_ids))
            .partition(|child| joined_spaces.contains(child));
    }
    spaces
}

pub(super) fn was_joined(joined: &[RoomId], id: &str) -> bool {
    joined.iter().any(|room| room.as_ref() == id)
}

pub fn space_children(space_id: &RoomId) -> Vec<SpaceChild> {
    let data = data();
    let via = [DEMO_VIA.to_owned()];
    let children: Vec<&String> = if let Some(space) = data
        .spaces
        .iter()
        .find(|space| space.id() == space_id.as_ref())
    {
        space.children().collect()
    } else if let Some(entry) = data
        .unjoined
        .iter()
        .find(|entry| entry.space && entry.id == space_id.as_ref())
    {
        entry.rooms.iter().chain(&entry.spaces).collect()
    } else {
        Vec::new()
    };
    children
        .into_iter()
        .filter_map(|id| space_child(data, id, &via))
        .collect()
}

fn space_child(data: &DemoData, id: &str, via: &[String]) -> Option<SpaceChild> {
    if let Some(room) = data.rooms.iter().find(|room| room.id == id) {
        return Some(room.to_child(via));
    }
    if let Some(space) = data.spaces.iter().find(|space| space.id() == id) {
        return Some(space.to_child(via));
    }
    data.unjoined
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.to_child(via))
}

pub fn alias_target(alias: &str) -> Option<RoomId> {
    let data = data();
    data.rooms
        .iter()
        .find(|room| room.alias.as_deref() == Some(alias))
        .map(|room| RoomId::new(&room.id))
        .or_else(|| {
            data.unjoined
                .iter()
                .find(|entry| entry.alias() == Some(alias))
                .map(|entry| RoomId::new(&entry.id))
        })
}

pub fn room_card(room_id: &RoomId) -> Option<RoomPreview> {
    let data = data();
    let via = [DEMO_VIA.to_owned()];
    if let Some(child) = space_child(data, room_id, &via) {
        return Some(preview_of(child));
    }
    data.invites
        .iter()
        .find(|entry| entry.id == room_id.as_ref())
        .map(InviteDto::to_preview)
}

fn preview_of(child: SpaceChild) -> RoomPreview {
    RoomPreview {
        is_space: matches!(child.kind, ChildKind::Space { .. }),
        id: child.id,
        name: child.name,
        alias: child.alias,
        topic: child.topic,
        avatar_mxc: child.avatar_mxc,
        member_count: child.member_count,
        join_rule: Some(child.join_rule),
        membership: PreviewMembership::Outsider,
    }
}

pub fn is_unjoined_avatar(mxc: &str) -> bool {
    data()
        .unjoined
        .iter()
        .any(|entry| entry.avatar() == Some(mxc))
}

pub fn is_member(room_id: &RoomId, user_id: &str) -> bool {
    room_members(room_id)
        .iter()
        .any(|(member, _)| member == user_id)
}

pub fn successor_of(room_id: &RoomId) -> Option<RoomId> {
    linked_room(
        room_id,
        |room| room.successor.as_deref(),
        |entry| entry.successor.as_deref(),
    )
}

pub fn predecessor_of(room_id: &RoomId) -> Option<RoomId> {
    linked_room(
        room_id,
        |room| room.predecessor.as_deref(),
        |entry| entry.predecessor.as_deref(),
    )
}

fn linked_room(
    room_id: &RoomId,
    joined: impl Fn(&RoomDto) -> Option<&str>,
    unjoined: impl Fn(&UnjoinedDto) -> Option<&str>,
) -> Option<RoomId> {
    let data = data();
    let linked = match data.rooms.iter().find(|room| room.id == room_id.as_ref()) {
        Some(room) => joined(room),
        None => data
            .unjoined
            .iter()
            .find(|entry| entry.id == room_id.as_ref())
            .and_then(unjoined),
    };
    linked.map(RoomId::new)
}
