use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::data;
use super::people::person_name;
use super::sent::minutes_before;
use super::spaces::{DEMO_VIA, room_link, was_joined};
use crate::adapters::demo::dto::{InviteDto, UnjoinedDto, quiet_room};
use crate::adapters::demo::{calls, invite_people};
use crate::domain::departure::{Removal, RemovalKind};
use crate::domain::invite::Invite;
use crate::domain::member_name::{MemberName, NamedUser};
use crate::domain::message::{MessagePreviewKind, RichText};
use crate::domain::room::{NotifyMode, Room, RoomId, Space, SpacePowers};
use crate::domain::room_info::RoomAbout;
use crate::domain::user_info::{UserId, localpart};
use crate::util::unix_now_ms;

#[derive(Default, Clone)]
pub struct RoomOverrides {
    pub left: HashSet<String>,
    pub notify: HashMap<String, NotifyMode>,
    pub read: HashSet<String>,
    pub started_chats: HashMap<String, String>,
    pub declined: HashSet<String>,
    pub withdrawn: HashSet<String>,
    pub held_back: HashSet<String>,
    pub arrivals: HashMap<String, Arrived>,
    pub created: Vec<Created>,
    pub created_children: HashMap<String, Vec<String>>,
    pub removed: HashMap<String, Removed>,
    pub forgotten: HashSet<String>,
}

#[derive(Clone)]
pub struct Removed {
    pub kind: RemovalKind,
    pub by: String,
    pub reason: Option<String>,
    pub at_ms: u64,
}

impl Removed {
    pub fn now(kind: RemovalKind, by: &str, reason: Option<&str>) -> Self {
        Self::ago(kind, by, reason, 0)
    }

    pub fn ago(kind: RemovalKind, by: &str, reason: Option<&str>, minutes: u64) -> Self {
        Self {
            kind,
            by: by.to_owned(),
            reason: reason.map(ToOwned::to_owned),
            at_ms: minutes_before(unix_now_ms(), minutes),
        }
    }
}

#[derive(Clone)]
pub struct Created {
    pub id: String,
    pub space: bool,
    pub name: String,
    pub topic: Option<String>,
    pub alias: Option<String>,
    pub encrypted: bool,
    pub at_ms: u64,
}

#[derive(Clone)]
pub struct Arrived {
    pub sender: Option<NamedUser>,
    pub kind: MessagePreviewKind,
    pub body: RichText,
    pub highlight: bool,
    pub at_ms: u64,
}

impl Arrived {
    pub fn now(
        sender: Option<NamedUser>,
        kind: MessagePreviewKind,
        body: RichText,
        highlight: bool,
    ) -> Self {
        Self {
            sender,
            kind,
            body,
            highlight,
            at_ms: unix_now_ms(),
        }
    }
}

pub fn rooms_with(joined: &[RoomId], overrides: &RoomOverrides) -> Vec<Arc<Room>> {
    let now = unix_now_ms();
    let data = data();
    data.rooms
        .iter()
        .map(|room| room.to_room(now, overrides.notify.get(&room.id).copied()))
        .chain(
            data.unjoined
                .iter()
                .filter(|entry| !entry.space && was_joined(joined, &entry.id))
                .map(|entry| {
                    let mut room = entry.to_room(now);
                    if let Some(notify) = overrides.notify.get(&entry.id) {
                        room.notify = *notify;
                    }
                    room
                }),
        )
        .chain(
            overrides
                .started_chats
                .iter()
                .map(|(room_id, user_id)| started_chat(room_id, user_id, now)),
        )
        .chain(
            data.invites
                .iter()
                .filter(|entry| !entry.space && was_joined(joined, &entry.id))
                .map(|entry| entry.to_room(now)),
        )
        .chain(
            overrides
                .created
                .iter()
                .filter(|created| !created.space)
                .map(created_room),
        )
        .filter(|room| !overrides.left.contains(room.id.as_ref()))
        .map(|room| arrived_when_marked(room, &overrides.arrivals))
        .map(|room| read_when_marked(room, &overrides.read))
        .map(calls::shaped)
        .map(Arc::new)
        .collect()
}

fn started_chat(room_id: &str, user_id: &str, now: u64) -> Room {
    let name = person_name(user_id).unwrap_or_else(|| localpart(user_id).to_owned());
    Room {
        avatar_mxc: Some(user_id.to_owned()),
        is_direct: true,
        direct_partner: Some(UserId::new(user_id)),
        is_encrypted: true,
        member_count: 2,
        ..quiet_room(RoomId::new(room_id), name, now)
    }
}

fn created_room(created: &Created) -> Room {
    Room {
        topic: created.topic.clone(),
        canonical_alias: created.alias.clone(),
        is_encrypted: created.encrypted,
        ..quiet_room(
            RoomId::new(&created.id),
            created.name.clone(),
            created.at_ms,
        )
    }
}

pub(super) fn created_space(created: &Created) -> Space {
    Space {
        id: created.id.clone(),
        name: created.name.clone(),
        avatar_mxc: None,
        member_count: 1,
        child_room_ids: Vec::new(),
        child_space_ids: Vec::new(),
        order: None,
        powers: SpacePowers {
            invite: invite_people::may_invite(),
            add_children: true,
        },
        alert: false,
        mention: false,
        hint: false,
    }
}

pub fn created_room_id(serial: u64) -> String {
    format!("!created-{serial}:{DEMO_VIA}")
}

pub fn created_about(created: &Created) -> RoomAbout {
    RoomAbout {
        joined_at: Some(created.at_ms),
        link: room_link(&RoomId::new(&created.id), created.alias.as_deref()),
        topic: created.topic.clone().map(RichText::plain),
    }
}

pub fn alias_localparts() -> impl Iterator<Item = &'static str> {
    let data = data();
    data.rooms
        .iter()
        .filter_map(|room| room.alias.as_deref())
        .chain(data.unjoined.iter().filter_map(UnjoinedDto::alias))
        .filter_map(|alias| {
            let local = alias.strip_prefix('#')?;
            Some(local.split_once(':').map_or(local, |(local, _)| local))
        })
}

pub fn removals_with(overrides: &RoomOverrides) -> Vec<Arc<Removal>> {
    let data = data();
    let mut removals: Vec<Arc<Removal>> = overrides
        .removed
        .iter()
        .filter(|(room_id, _)| !overrides.forgotten.contains(*room_id))
        .filter_map(|(room_id, removed)| {
            let room = data.rooms.iter().find(|room| room.id == *room_id)?;
            let shown = room.to_room(unix_now_ms(), None);
            Some(Arc::new(Removal {
                id: shown.id,
                name: shown.display_name,
                avatar_mxc: shown.avatar_mxc,
                kind: removed.kind,
                is_space: false,
                is_direct: shown.is_direct,
                by: NamedUser::new(
                    removed.by.as_str(),
                    MemberName::new(person_name(&removed.by).as_deref(), false),
                ),
                reason: removed.reason.clone(),
                removed_ts: removed.at_ms,
            }))
        })
        .collect();
    removals.sort_by(|a, b| {
        b.removed_ts
            .cmp(&a.removed_ts)
            .then_with(|| a.id.as_ref().cmp(b.id.as_ref()))
    });
    removals
}

pub fn invites_with(joined: &[RoomId], overrides: &RoomOverrides) -> Vec<Arc<Invite>> {
    let now = unix_now_ms();
    data()
        .invites
        .iter()
        .filter(|entry| !was_joined(joined, &entry.id))
        .filter(|entry| {
            [
                &overrides.declined,
                &overrides.withdrawn,
                &overrides.held_back,
            ]
            .iter()
            .all(|set| !set.contains(&entry.id))
        })
        .map(|entry| Arc::new(entry.to_invite(now, inviter_of(entry))))
        .collect()
}

fn inviter_of(entry: &InviteDto) -> Option<NamedUser> {
    let user_id = entry.inviter.as_deref()?;
    let name = MemberName::new(person_name(user_id).as_deref(), false);
    Some(NamedUser::new(user_id, name))
}

pub fn started_chat_id(user_id: &str) -> String {
    format!("!chat-{}:{DEMO_VIA}", localpart(user_id))
}

fn arrived_when_marked(mut room: Room, arrivals: &HashMap<String, Arrived>) -> Room {
    let Some(arrived) = arrivals.get(room.id.as_ref()) else {
        return room;
    };
    room.has_activity = true;
    room.has_unread |= match room.notify {
        NotifyMode::AllMessages => true,
        NotifyMode::MentionsOnly => arrived.highlight,
        NotifyMode::Muted => false,
    };
    room.has_mentions |= arrived.highlight;
    room.last_message_sender.clone_from(&arrived.sender);
    room.last_message_kind = arrived.kind;
    room.last_message_body = arrived.body.clone();
    room.last_message_service = None;
    room.last_message_is_own = false;
    room.last_message_edited = false;
    room.last_activity_ts = arrived.at_ms;
    room
}

fn read_when_marked(mut room: Room, read: &HashSet<String>) -> Room {
    if read.contains(room.id.as_ref()) {
        room.has_unread = false;
        room.has_mentions = false;
        room.has_activity = false;
    }
    room
}
