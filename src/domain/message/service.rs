use crate::domain::member_name::NamedUser;
use crate::domain::room_state::{GuestAccess, HistoryVisibility, PowerChange};
use crate::domain::space_index::JoinRule;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceEvent {
    Joined,
    Left,
    Invited {
        target: NamedUser,
    },
    InvitationAccepted,
    InvitationRejected,
    InvitationRevoked {
        target: NamedUser,
    },
    Kicked {
        target: NamedUser,
        reason: Option<String>,
    },
    Banned {
        target: NamedUser,
        reason: Option<String>,
    },
    Unbanned {
        target: NamedUser,
    },
    Knocked,
    KnockAccepted {
        target: NamedUser,
    },
    KnockRetracted,
    KnockDenied {
        target: NamedUser,
    },
    ThirdPartyInvited {
        name: String,
    },
    ThirdPartyInviteRevoked {
        name: String,
    },
    DisplayNameSet {
        name: String,
    },
    DisplayNameChanged {
        name: String,
    },
    DisplayNameRemoved,
    AvatarChanged,
    RoomNameChanged {
        name: String,
    },
    RoomNameRemoved,
    RoomTopicChanged,
    RoomAvatarChanged,
    RoomCreated,
    UpgradedRoomCreated,
    RoomUpgraded,
    EncryptionEnabled,
    JoinRuleChanged(JoinRule),
    HistoryVisibilityChanged(HistoryVisibility),
    GuestAccessChanged(GuestAccess),
    ServerAclChanged,
    MainAddressSet {
        alias: String,
    },
    MainAddressRemoved,
    AlternativeAddressesChanged,
    MessagePinned,
    MessageUnpinned,
    PinnedMessagesChanged,
    PowerLevelsChanged(PowerChange),
    CallStarted,
    CallNotification {
        video: bool,
        declined_by: Option<NamedUser>,
    },
    VerificationRequested {
        target: NamedUser,
    },
}

impl ServiceEvent {
    pub fn target(&self) -> Option<&NamedUser> {
        match self {
            Self::Invited { target }
            | Self::InvitationRevoked { target }
            | Self::Kicked { target, .. }
            | Self::Banned { target, .. }
            | Self::Unbanned { target }
            | Self::KnockAccepted { target }
            | Self::KnockDenied { target }
            | Self::VerificationRequested { target }
            | Self::CallNotification {
                declined_by: Some(target),
                ..
            } => Some(target),
            Self::PowerLevelsChanged(change) => change.target(),
            _ => None,
        }
    }

    pub fn target_mut(&mut self) -> Option<&mut NamedUser> {
        match self {
            Self::Invited { target }
            | Self::InvitationRevoked { target }
            | Self::Kicked { target, .. }
            | Self::Banned { target, .. }
            | Self::Unbanned { target }
            | Self::KnockAccepted { target }
            | Self::KnockDenied { target }
            | Self::VerificationRequested { target }
            | Self::CallNotification {
                declined_by: Some(target),
                ..
            } => Some(target),
            Self::PowerLevelsChanged(change) => change.target_mut(),
            _ => None,
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Kicked { reason, .. } | Self::Banned { reason, .. } => reason.as_deref(),
            _ => None,
        }
    }
}
