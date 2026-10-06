use std::cmp::Reverse;

use crate::domain::message::RichText;
use crate::domain::user_info::localpart;

pub const ADMIN_LEVEL: i64 = 100;
pub const MODERATOR_LEVEL: i64 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemberRole {
    Member,
    Moderator,
    Admin,
    Owner,
}

impl MemberRole {
    pub fn for_level(level: i64) -> Self {
        if level >= ADMIN_LEVEL {
            Self::Admin
        } else if level >= MODERATOR_LEVEL {
            Self::Moderator
        } else {
            Self::Member
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RosterSection {
    Joined,
    Invited,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterMember {
    pub user_id: String,
    pub display_name: Option<String>,
    pub avatar_mxc: Option<String>,
    pub role: MemberRole,
    pub section: RosterSection,
}

fn shown_name<'a>(display_name: Option<&'a str>, user_id: &'a str) -> &'a str {
    display_name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| localpart(user_id))
}

impl RosterMember {
    pub fn label(&self) -> &str {
        shown_name(self.display_name.as_deref(), &self.user_id)
    }

    pub fn matches(&self, query: &MemberQuery) -> bool {
        query.is_empty()
            || self.user_id.to_lowercase().contains(&query.0)
            || self
                .display_name
                .as_deref()
                .is_some_and(|name| name.to_lowercase().contains(&query.0))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemberQuery(String);

impl MemberQuery {
    pub fn new(raw: &str) -> Self {
        Self(raw.trim().to_lowercase())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

pub fn sort_roster(members: &mut [RosterMember]) {
    members.sort_by_cached_key(|member| {
        (
            member.section,
            Reverse(member.role),
            member.label().to_lowercase(),
            member.user_id.clone(),
        )
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reader {
    pub user_id: String,
    pub display_name: Option<String>,
    pub avatar_mxc: Option<String>,
    pub role: MemberRole,
}

impl Reader {
    pub fn unknown(user_id: String) -> Self {
        Self {
            user_id,
            display_name: None,
            avatar_mxc: None,
            role: MemberRole::Member,
        }
    }

    pub fn label(&self) -> &str {
        shown_name(self.display_name.as_deref(), &self.user_id)
    }
}

pub fn sort_readers(readers: &mut [Reader]) {
    readers.sort_by_cached_key(|reader| (reader.label().to_lowercase(), reader.user_id.clone()));
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomAbout {
    pub joined_at: Option<u64>,
    pub link: String,
    pub topic: Option<RichText>,
}
