use crate::domain::member_name::MemberName;
use crate::domain::user_info::{RoomMembership, UserId};

pub const PEOPLE_SEARCH_LIMIT: u32 = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub user_id: UserId,
    pub name: MemberName,
    pub avatar_mxc: Option<String>,
    pub membership: RoomMembership,
}

impl Person {
    pub fn invitable(&self) -> bool {
        !matches!(
            self.membership,
            RoomMembership::Joined | RoomMembership::Invited | RoomMembership::Banned
        )
    }

    pub fn shown(&self) -> String {
        self.name.shown(&self.user_id)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeopleFound {
    pub people: Vec<Person>,
    pub limited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeopleSearchFailure {
    RateLimited,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteFailure {
    Blocked,
    Refused,
    RateLimited,
    Failed,
}

pub type SearchResult = Result<PeopleFound, PeopleSearchFailure>;

pub type InviteResult = Result<(), InviteFailure>;
