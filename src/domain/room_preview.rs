use crate::domain::room::RoomId;
use crate::domain::space_index::JoinRule;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewMembership {
    Outsider,
    Joined,
    Invited,
    Knocked,
    Banned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomPreview {
    pub id: RoomId,
    pub name: String,
    pub alias: Option<String>,
    pub topic: Option<String>,
    pub avatar_mxc: Option<String>,
    pub member_count: u64,
    pub is_space: bool,
    pub join_rule: Option<JoinRule>,
    pub membership: PreviewMembership,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewFailure {
    NoSuchRoom,
    Unavailable,
    Failed,
}

pub type PreviewResult = Result<RoomPreview, PreviewFailure>;
