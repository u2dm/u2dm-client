use super::ActionOutcome;
use crate::app::user_info::{DmOutcome, ReadOutcome, VerifyOutcome};
use crate::domain::invite_people::{InviteResult, SearchResult};
use crate::domain::presence::LookupResult;
use crate::domain::room::RoomId;
use crate::domain::room_info::{Reader, RosterMember};
use crate::domain::user_info::{GlobalProfile, IgnoreChange, Moderation, UserId, UserTrust};
use crate::error::AppError;

pub(in crate::app) enum PresenceEvent {
    Disabled {
        session: u64,
    },
    Looked {
        watch: u64,
        user_id: UserId,
        result: LookupResult,
    },
}

impl PresenceEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::Disabled { .. } => "PresenceDisabled",
            Self::Looked { .. } => "PresenceLooked",
        }
    }
}

pub(in crate::app) enum InvitePeopleEvent {
    Found {
        generation: u64,
        outcome: SearchResult,
    },
    AvatarsReady {
        generation: u64,
        ready: usize,
    },
    Sent {
        batch: u64,
        outcomes: Vec<(UserId, InviteResult)>,
    },
}

impl InvitePeopleEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::Found { .. } => "PeopleFound",
            Self::AvatarsReady { .. } => "InviteeAvatarsReady",
            Self::Sent { .. } => "InvitesSent",
        }
    }
}

pub(in crate::app) enum MentionsEvent {
    RosterLoaded {
        request: u64,
        joined_count: u64,
        roster: Result<Vec<RosterMember>, AppError>,
    },
    AvatarsReady {
        request: u64,
        ready: usize,
    },
}

impl MentionsEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::RosterLoaded { .. } => "MentionRosterLoaded",
            Self::AvatarsReady { .. } => "MentionAvatarsReady",
        }
    }
}

pub(in crate::app) enum ReadersEvent {
    Named { request: u64, readers: Vec<Reader> },
    AvatarsReady { request: u64, ready: usize },
}

impl ReadersEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::Named { .. } => "ReadersNamed",
            Self::AvatarsReady { .. } => "ReaderAvatarsReady",
        }
    }
}

pub(in crate::app) enum UserInfoEvent {
    Read {
        generation: u64,
        outcome: ReadOutcome,
    },
    PronounsFetched {
        generation: u64,
        pronouns: Vec<String>,
    },
    ProfileFetched {
        generation: u64,
        profile: Option<GlobalProfile>,
    },
    AvatarReady {
        generation: u64,
    },
    DmStarted {
        user_id: UserId,
        name: String,
        outcome: DmOutcome,
    },
    TrustChanged {
        generation: u64,
        trust: UserTrust,
    },
    VerifySettled {
        user_id: UserId,
        name: String,
        outcome: VerifyOutcome,
    },
    IgnoreSettled {
        user_id: UserId,
        name: String,
        change: IgnoreChange,
        outcome: ActionOutcome,
    },
    ModerationSettled {
        room_id: RoomId,
        user_id: UserId,
        name: String,
        action: Moderation,
        outcome: ActionOutcome,
    },
}

impl UserInfoEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::Read { .. } => "UserInfoRead",
            Self::PronounsFetched { .. } => "UserPronounsFetched",
            Self::ProfileFetched { .. } => "UserProfileFetched",
            Self::AvatarReady { .. } => "UserInfoAvatarReady",
            Self::DmStarted { .. } => "DmStarted",
            Self::TrustChanged { .. } => "UserTrustChanged",
            Self::VerifySettled { .. } => "UserVerifySettled",
            Self::IgnoreSettled { .. } => "IgnoreSettled",
            Self::ModerationSettled { .. } => "ModerationSettled",
        }
    }
}
