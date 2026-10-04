use std::{fmt, ops};

use crate::domain::room::RoomId;
use crate::domain::room_info::MemberRole;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserId(String);

impl UserId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn localpart(&self) -> &str {
        localpart(&self.0)
    }
}

impl ops::Deref for UserId {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for UserId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomMembership {
    Joined,
    Invited,
    Knocking,
    Left,
    Banned,
    Outside,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityTrust {
    Verified,
    Unverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreChange {
    Ignore,
    Unignore,
}

impl IgnoreChange {
    pub fn ignores(self) -> bool {
        matches!(self, Self::Ignore)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moderation {
    Kick,
    Ban,
    Unban,
}

impl Moderation {
    pub fn leaves(self) -> RoomMembership {
        match self {
            Self::Kick | Self::Unban => RoomMembership::Left,
            Self::Ban => RoomMembership::Banned,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModerationPowers {
    pub kick: bool,
    pub ban: bool,
    pub unban: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pronouns {
    Known(Vec<String>),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserProfile {
    pub user_id: UserId,
    pub display_name: Option<String>,
    pub avatar_mxc: Option<String>,
    pub role: MemberRole,
    pub membership: RoomMembership,
    pub trust: IdentityTrust,
    pub is_self: bool,
    pub pronouns: Pronouns,
    pub link: String,
    pub direct_room: Option<RoomId>,
    pub ignored: bool,
    pub powers: ModerationPowers,
}

impl UserProfile {
    pub fn placeholder(user_id: UserId) -> Self {
        Self {
            user_id,
            display_name: None,
            avatar_mxc: None,
            role: MemberRole::Member,
            membership: RoomMembership::Unknown,
            trust: IdentityTrust::Unverified,
            is_self: false,
            pronouns: Pronouns::Known(Vec::new()),
            link: String::new(),
            direct_room: None,
            ignored: false,
            powers: ModerationPowers::default(),
        }
    }

    pub fn label(&self) -> &str {
        self.display_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| self.user_id.localpart())
    }

    pub fn wants_global_profile(&self) -> bool {
        self.display_name.is_none()
            && matches!(
                self.membership,
                RoomMembership::Outside | RoomMembership::Unknown
            )
    }

    pub fn wants_pronouns(&self) -> bool {
        self.pronouns == Pronouns::Unknown
    }

    pub fn offers(&self, action: Moderation) -> bool {
        match action {
            Moderation::Kick => {
                self.powers.kick
                    && matches!(
                        self.membership,
                        RoomMembership::Joined | RoomMembership::Invited
                    )
            }
            Moderation::Ban => {
                self.powers.ban
                    && matches!(
                        self.membership,
                        RoomMembership::Joined
                            | RoomMembership::Invited
                            | RoomMembership::Left
                            | RoomMembership::Knocking
                    )
            }
            Moderation::Unban => self.powers.unban && self.membership == RoomMembership::Banned,
        }
    }

    pub fn adopt_global(&mut self, global: GlobalProfile) {
        if self.display_name.is_none() {
            self.display_name = global.display_name;
        }
        if self.avatar_mxc.is_none() {
            self.avatar_mxc = global.avatar_mxc;
        }
        self.pronouns = Pronouns::Known(global.pronouns);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalProfile {
    pub display_name: Option<String>,
    pub avatar_mxc: Option<String>,
    pub pronouns: Vec<String>,
}

pub fn localpart(user_id: &str) -> &str {
    let name = user_id.strip_prefix('@').unwrap_or(user_id);
    name.split_once(':').map_or(name, |(local, _)| local)
}
