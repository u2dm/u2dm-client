use crate::domain::room::RoomId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinRule {
    Public,
    Restricted { allowed: Vec<RoomId> },
    KnockRestricted { allowed: Vec<RoomId> },
    Knock,
    Invite,
    Private,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildKind {
    Room,
    Space { children: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceChild {
    pub id: RoomId,
    pub name: String,
    pub alias: Option<String>,
    pub topic: Option<String>,
    pub avatar_mxc: Option<String>,
    pub member_count: u64,
    pub join_rule: JoinRule,
    pub kind: ChildKind,
    pub via: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HierarchyPage {
    pub children: Vec<SpaceChild>,
    pub next: Option<String>,
}
