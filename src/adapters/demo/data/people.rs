use std::collections::HashMap;

use super::spaces::{DEMO_VIA, MATRIX_TO};
use super::{OWN_NAME_IN_TIMELINES, data, own_user, pronouns};
use crate::adapters::demo::dto::{DemoData, InviteDto, UnjoinedDto};
use crate::adapters::demo::names::{self, NameBook};
use crate::domain::member_name::MemberName;
use crate::domain::room::RoomId;
use crate::domain::room_info::{MemberRole, Reader, RosterMember, RosterSection, sort_roster};
use crate::domain::user_info::{
    IdentityTrust, ModerationPowers, Pronouns, RoomMembership, UserId, UserProfile,
};

pub(super) fn room_members(room_id: &RoomId) -> Vec<(String, RosterSection)> {
    let data = data();
    let fixture = data.rooms.iter().find(|room| room.id == room_id.as_ref());
    let invite = data
        .invites
        .iter()
        .find(|entry| entry.id == room_id.as_ref());
    let joined_count = fixture
        .map(|room| room.members)
        .or_else(|| {
            data.unjoined
                .iter()
                .find(|entry| entry.id == room_id.as_ref())
                .map(UnjoinedDto::joined_members)
        })
        .or_else(|| invite.map(InviteDto::joined_members));
    let Some(joined_count) = joined_count else {
        return Vec::new();
    };
    let mut joined: Vec<String> = vec![own_user().to_owned()];
    joined.extend(invite.and_then(|entry| entry.inviter.clone()));
    let mut invited: Vec<String> = Vec::new();
    if let Some(room) = fixture {
        let senders = data
            .timelines
            .get(&room.id)
            .into_iter()
            .flatten()
            .map(|message| message.author().0.to_owned());
        let added = names::members(room_id)
            .into_iter()
            .map(|(user_id, _)| user_id.to_owned());
        for user_id in senders.chain(room.roles.keys().cloned()).chain(added) {
            if !joined.contains(&user_id) {
                joined.push(user_id);
            }
        }
        invited.extend(room.invited.iter().cloned());
    }
    let guests = usize::try_from(joined_count)
        .unwrap_or(usize::MAX)
        .saturating_sub(joined.len());
    joined
        .into_iter()
        .map(|user_id| (user_id, RosterSection::Joined))
        .chain((1..=guests).map(|n| (guest_id(n), RosterSection::Joined)))
        .chain(
            invited
                .into_iter()
                .map(|user_id| (user_id, RosterSection::Invited)),
        )
        .collect()
}

pub fn names_in(room_id: &RoomId, referenced: impl IntoIterator<Item = String>) -> NameBook {
    let data = data();
    let mut known = people_names(data);
    for message in data.timelines.get(room_id.as_ref()).into_iter().flatten() {
        let (sender, name) = message.author();
        if names_a_person(name) {
            known.insert(sender.to_owned(), name.to_owned());
        }
    }
    for (user_id, name) in names::members(room_id) {
        if let Some(name) = name {
            known.insert(user_id.to_owned(), name.to_owned());
        }
    }
    let users = room_members(room_id)
        .into_iter()
        .map(|(user_id, _)| user_id)
        .chain(referenced);
    NameBook::new(users.filter_map(|user_id| {
        let name = known
            .get(&user_id)
            .cloned()
            .or_else(|| guest_name(&user_id))?;
        Some((user_id, name))
    }))
}

pub fn roster(room_id: &RoomId, avatar_of: impl Fn(&str) -> String) -> Vec<RosterMember> {
    let roles: HashMap<String, MemberRole> = data()
        .rooms
        .iter()
        .find(|room| room.id == room_id.as_ref())
        .map(|room| {
            room.roles
                .iter()
                .map(|(user, role)| (user.clone(), role.to_role()))
                .collect()
        })
        .unwrap_or_default();
    let book = names_in(room_id, []);
    let mut roster: Vec<RosterMember> = room_members(room_id)
        .into_iter()
        .map(|(user_id, section)| RosterMember {
            name: book.of(&user_id),
            avatar_mxc: Some(avatar_of(&user_id)),
            role: roles.get(&user_id).copied().unwrap_or(MemberRole::Member),
            section,
            user_id,
        })
        .collect();
    sort_roster(&mut roster);
    roster
}

pub fn profile(
    room_id: &RoomId,
    user_id: &UserId,
    avatar_of: impl Fn(&str) -> String,
) -> UserProfile {
    let member = roster(room_id, &avatar_of)
        .into_iter()
        .find(|member| member.user_id == user_id.as_ref());
    let membership = match member.as_ref().map(|member| member.section) {
        Some(RosterSection::Joined) => RoomMembership::Joined,
        Some(RosterSection::Invited) => RoomMembership::Invited,
        None => RoomMembership::Outside,
    };
    UserProfile {
        user_id: user_id.clone(),
        name: member
            .as_ref()
            .map(|member| member.name.clone())
            .unwrap_or_default(),
        avatar_mxc: member.as_ref().and_then(|member| member.avatar_mxc.clone()),
        role: member
            .as_ref()
            .map_or(MemberRole::Member, |member| member.role),
        membership,
        trust: IdentityTrust::Unverified,
        is_self: user_id.as_ref() == own_user(),
        pronouns: Pronouns::Known(pronouns(user_id)),
        link: user_link(user_id),
        direct_room: None,
        ignored: false,
        powers: ModerationPowers::default(),
    }
}

pub fn readers(
    room_id: &RoomId,
    user_ids: &[String],
    avatar_of: impl Fn(&str) -> String,
) -> Vec<Reader> {
    let members = roster(room_id, &avatar_of);
    let book = names_in(room_id, user_ids.iter().cloned());
    user_ids
        .iter()
        .map(|user_id| {
            members
                .iter()
                .find(|member| member.user_id == *user_id)
                .map_or_else(
                    || Reader {
                        user_id: user_id.clone(),
                        name: book.of(user_id),
                        avatar_mxc: Some(avatar_of(user_id)),
                        role: role_of(room_id, user_id),
                    },
                    |member| Reader {
                        user_id: member.user_id.clone(),
                        name: member.name.clone(),
                        avatar_mxc: member.avatar_mxc.clone(),
                        role: member.role,
                    },
                )
        })
        .collect()
}

pub fn role_of(room_id: &RoomId, user_id: &str) -> MemberRole {
    data()
        .rooms
        .iter()
        .find(|room| room.id == room_id.as_ref())
        .and_then(|room| room.roles.get(user_id))
        .map_or(MemberRole::Member, |role| role.to_role())
}

pub fn person_name(user_id: &str) -> Option<String> {
    people_names(data())
        .remove(user_id)
        .or_else(|| guest_name(user_id))
}

pub fn user_link(user_id: &str) -> String {
    format!("{MATRIX_TO}{user_id}")
}

pub fn direct_room_with(user_id: &str) -> Option<RoomId> {
    let own = own_user();
    data()
        .rooms
        .iter()
        .filter(|room| room.is_direct())
        .find_map(|room| {
            let room_id = RoomId::new(&room.id);
            let others: Vec<String> = roster(&room_id, str::to_owned)
                .into_iter()
                .map(|member| member.user_id)
                .filter(|member| member != own)
                .collect();
            (others == [user_id]).then_some(room_id)
        })
}

pub fn guest_user(n: usize) -> String {
    guest_id(n)
}

const GUEST_PREFIX: &str = "@guest-";

fn guest_id(n: usize) -> String {
    format!("{GUEST_PREFIX}{n}:{DEMO_VIA}")
}

fn guest_name(user_id: &str) -> Option<String> {
    let rest = user_id.strip_prefix(GUEST_PREFIX)?;
    let (n, _) = rest.split_once(':')?;
    Some(format!("Guest {n}"))
}

fn names_a_person(name: &str) -> bool {
    name != OWN_NAME_IN_TIMELINES && !name.is_empty()
}

pub fn directory_people() -> Vec<(String, String)> {
    let mut people: Vec<(String, String)> = people_names(data()).into_iter().collect();
    people.sort();
    people
}

pub fn named_member(
    user_id: &str,
    role: MemberRole,
    section: RosterSection,
    avatar_of: impl Fn(&str) -> String,
) -> RosterMember {
    RosterMember {
        user_id: user_id.to_owned(),
        name: MemberName::new(person_name(user_id).as_deref(), false),
        avatar_mxc: Some(avatar_of(user_id)),
        role,
        section,
    }
}

fn people_names(data: &DemoData) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for message in data.timelines.values().flatten() {
        let (sender, name) = message.author();
        if names_a_person(name) {
            names
                .entry(sender.to_owned())
                .or_insert_with(|| name.to_owned());
        }
    }
    names
}
