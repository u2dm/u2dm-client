use crate::domain::member_name::NamedUser;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionsOf {
    You(String),
    Member(NamedUser),
}

impl SessionsOf {
    pub fn user_id(&self) -> &str {
        match self {
            Self::You(user_id) => user_id,
            Self::Member(user) => &user.user_id,
        }
    }

    fn yours(&self) -> bool {
        matches!(self, Self::You(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomAlert {
    NoLongerVerified(NamedUser),
    UnverifiedSessions(SessionsOf),
    IdentityReset(NamedUser),
}

impl RoomAlert {
    pub fn user_id(&self) -> &str {
        match self {
            Self::NoLongerVerified(user) | Self::IdentityReset(user) => &user.user_id,
            Self::UnverifiedSessions(sessions) => sessions.user_id(),
        }
    }

    fn rank(&self) -> u8 {
        match self {
            Self::NoLongerVerified(_) => 0,
            Self::UnverifiedSessions(_) => 1,
            Self::IdentityReset(_) => 2,
        }
    }

    fn urgency(&self) -> (u8, bool, &str) {
        let yours = matches!(self, Self::UnverifiedSessions(sessions) if sessions.yours());
        (self.rank(), !yours, self.user_id())
    }

    fn concerns(&self, other: &Self) -> bool {
        self.rank() == other.rank() && self.user_id() == other.user_id()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OwnIdentity {
    #[default]
    Unchanged,
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityAlert {
    OwnIdentityReset,
    Room(RoomAlert),
}

impl IdentityAlert {
    pub fn most_urgent(own: OwnIdentity, room: &[RoomAlert]) -> Option<Self> {
        if own == OwnIdentity::Reset {
            return Some(Self::OwnIdentityReset);
        }
        room.iter()
            .min_by(|a, b| a.urgency().cmp(&b.urgency()))
            .cloned()
            .map(Self::Room)
    }

    pub fn still_raised(&self, own: OwnIdentity, room: &[RoomAlert]) -> bool {
        match self {
            Self::OwnIdentityReset => own == OwnIdentity::Reset,
            Self::Room(alert) => room.iter().any(|raised| raised.concerns(alert)),
        }
    }

    pub fn concerns(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::OwnIdentityReset, Self::OwnIdentityReset) => true,
            (Self::Room(alert), Self::Room(other)) => alert.concerns(other),
            (Self::OwnIdentityReset, Self::Room(_)) | (Self::Room(_), Self::OwnIdentityReset) => {
                false
            }
        }
    }
}
