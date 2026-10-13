use crate::domain::member_name::NamedUser;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomAlert {
    NoLongerVerified(NamedUser),
    IdentityReset(NamedUser),
}

impl RoomAlert {
    pub fn user(&self) -> &NamedUser {
        match self {
            Self::NoLongerVerified(user) | Self::IdentityReset(user) => user,
        }
    }

    pub fn user_id(&self) -> &str {
        &self.user().user_id
    }

    fn rank(&self) -> u8 {
        match self {
            Self::NoLongerVerified(_) => 0,
            Self::IdentityReset(_) => 1,
        }
    }

    fn urgency(&self) -> (u8, &str) {
        (self.rank(), self.user_id())
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAlert {
    Unverified,
    NotSetUp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityAlert {
    OwnIdentityReset,
    Session(SessionAlert),
    Room(RoomAlert),
}

impl IdentityAlert {
    pub fn most_urgent(
        own: OwnIdentity,
        session: Option<SessionAlert>,
        room: &[RoomAlert],
    ) -> Option<Self> {
        if own == OwnIdentity::Reset {
            return Some(Self::OwnIdentityReset);
        }
        if let Some(session) = session {
            return Some(Self::Session(session));
        }
        room.iter()
            .min_by(|a, b| a.urgency().cmp(&b.urgency()))
            .cloned()
            .map(Self::Room)
    }

    pub fn still_raised(
        &self,
        own: OwnIdentity,
        session: Option<SessionAlert>,
        room: &[RoomAlert],
    ) -> bool {
        match self {
            Self::OwnIdentityReset => own == OwnIdentity::Reset,
            Self::Session(alert) => session == Some(*alert),
            Self::Room(alert) => room.iter().any(|raised| raised.concerns(alert)),
        }
    }

    pub fn concerns(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::OwnIdentityReset, Self::OwnIdentityReset) => true,
            (Self::Session(alert), Self::Session(other)) => alert == other,
            (Self::Room(alert), Self::Room(other)) => alert.concerns(other),
            (Self::OwnIdentityReset | Self::Session(_) | Self::Room(_), _) => false,
        }
    }
}
