use crate::domain::member_name::NamedUser;
use crate::domain::room_info::MemberRole;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HistoryVisibility {
    WorldReadable,
    #[default]
    Shared,
    Invited,
    Joined,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestAccess {
    CanJoin,
    Forbidden,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleChange {
    MadeAdmin,
    MadeModerator,
    NoLongerAdmin,
    NoLongerModerator,
}

impl RoleChange {
    pub fn between(before: MemberRole, after: MemberRole) -> Option<Self> {
        if before == after {
            return None;
        }
        match (before, after) {
            (_, MemberRole::Admin) => Some(Self::MadeAdmin),
            (_, MemberRole::Moderator) => Some(Self::MadeModerator),
            (MemberRole::Admin, MemberRole::Member) => Some(Self::NoLongerAdmin),
            (MemberRole::Moderator, MemberRole::Member) => Some(Self::NoLongerModerator),
            (MemberRole::Member | MemberRole::Owner, MemberRole::Member)
            | (_, MemberRole::Owner) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerChange {
    Role {
        target: NamedUser,
        change: RoleChange,
    },
    Level {
        target: NamedUser,
    },
    Levels,
}

impl PowerChange {
    pub fn of_user(user_id: &str, before: i64, after: i64) -> Self {
        let target = NamedUser::unnamed(user_id);
        match RoleChange::between(MemberRole::for_level(before), MemberRole::for_level(after)) {
            Some(change) => Self::Role { target, change },
            None => Self::Level { target },
        }
    }

    pub fn target(&self) -> Option<&NamedUser> {
        match self {
            Self::Role { target, .. } | Self::Level { target } => Some(target),
            Self::Levels => None,
        }
    }

    pub fn target_mut(&mut self) -> Option<&mut NamedUser> {
        match self {
            Self::Role { target, .. } | Self::Level { target } => Some(target),
            Self::Levels => None,
        }
    }
}
