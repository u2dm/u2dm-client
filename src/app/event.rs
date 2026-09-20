use std::path::PathBuf;

use super::establish::EstablishedSession;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::LoginActivity;
use crate::domain::auth::ServerInfo;
use crate::domain::media::PickedAttachment;
use crate::domain::room::RoomId;
use crate::domain::timeline::{
    AudioTrack, PaginationDirection, PaginationOutcome, TimelineAdvance, TimelineFocus,
};
use crate::domain::verification::VerificationEvent;
use crate::ports::matrix::{AuthenticatedSession, CleanupReport};

#[derive(Clone, Copy)]
pub(super) enum EndReason {
    UserLogout,
    Expired,
}

pub(super) struct AttachmentPicked {
    pub(super) pick: u64,
    pub(super) room_id: RoomId,
    pub(super) outcome: Result<PickedAttachment, UserMessage>,
}

pub(super) enum AppEvent {
    Session(SessionEvent),
    Timeline(TimelineEvent),
    SpaceOrderWriteFailed {
        op: u64,
        spaces: Vec<String>,
        error: String,
    },
    VerificationFlow(VerificationEvent),
    VerificationActionFailed(UserMessageKind),
    AttachmentPicked(Box<AttachmentPicked>),
    AttachmentSettled {
        submission: u64,
        failure: Option<UserMessage>,
    },
    AudioFetched {
        request: u64,
        outcome: Result<PathBuf, UserMessageKind>,
    },
    VideoFetched {
        request: u64,
        outcome: Result<PathBuf, UserMessageKind>,
    },
    SubmissionSettled {
        submission: i32,
        enqueue: Enqueue,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Enqueue {
    Accepted,
    Refused,
}

impl AppEvent {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Session(event) => event.label(),
            Self::Timeline(event) => event.label(),
            Self::SpaceOrderWriteFailed { .. } => "SpaceOrderWriteFailed",
            Self::VerificationFlow(_) => "VerificationFlow",
            Self::VerificationActionFailed(_) => "VerificationActionFailed",
            Self::AttachmentPicked(_) => "AttachmentPicked",
            Self::AttachmentSettled { .. } => "AttachmentSettled",
            Self::AudioFetched { .. } => "AudioFetched",
            Self::VideoFetched { .. } => "VideoFetched",
            Self::SubmissionSettled { .. } => "SubmissionSettled",
        }
    }
}

pub(super) enum TimelineEvent {
    Advanced {
        room_id: RoomId,
        generation: i32,
        advance: TimelineAdvance,
    },
    PaginationCompleted {
        room_id: RoomId,
        generation: i32,
        direction: PaginationDirection,
        outcome: PaginationOutcome,
    },
    Refocus {
        room_id: RoomId,
        generation: i32,
        focus: TimelineFocus,
    },
    AudioLocated {
        room_id: RoomId,
        generation: i32,
        request: u64,
        track: Option<Box<AudioTrack>>,
    },
}

impl TimelineEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::Advanced { .. } => "TimelineAdvanced",
            Self::PaginationCompleted { .. } => "TimelinePaginationCompleted",
            Self::Refocus { .. } => "RefocusTimeline",
            Self::AudioLocated { .. } => "AudioLocated",
        }
    }
}

pub(super) enum SessionEvent {
    RestoreProgress(LoginActivity),
    RestoreFailed(Option<UserMessage>),
    Restored(Box<AuthenticatedSession>),
    ServerDiscovered {
        attempt: u64,
        info: Box<ServerInfo>,
    },
    AuthActivity {
        attempt: u64,
        activity: LoginActivity,
    },
    AuthRejected {
        attempt: u64,
        message: UserMessage,
    },
    AuthCancelled {
        attempt: u64,
    },
    LoggedIn {
        attempt: u64,
        established: Box<EstablishedSession>,
    },
    LoginUnresolved(UserMessage),
    Resumed {
        attempt: u64,
        capability: Box<AuthenticatedSession>,
    },
    ErasingLocalState {
        session: u64,
    },
    LocalStateCleared {
        session: u64,
        reason: EndReason,
        report: CleanupReport,
    },
    TokensNotPersisted,
    UserAvatar(Option<PathBuf>),
    Suspended,
    Expired,
}

impl SessionEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::RestoreProgress(_) => "RestoreProgress",
            Self::RestoreFailed(_) => "RestoreFailed",
            Self::Restored(_) => "Restored",
            Self::ServerDiscovered { .. } => "ServerDiscovered",
            Self::AuthActivity { .. } => "AuthActivity",
            Self::AuthRejected { .. } => "AuthRejected",
            Self::AuthCancelled { .. } => "AuthCancelled",
            Self::LoggedIn { .. } => "LoggedIn",
            Self::LoginUnresolved(_) => "LoginUnresolved",
            Self::Resumed { .. } => "Resumed",
            Self::ErasingLocalState { .. } => "ErasingLocalState",
            Self::LocalStateCleared { .. } => "LocalStateCleared",
            Self::TokensNotPersisted => "TokensNotPersisted",
            Self::UserAvatar(_) => "UserAvatar",
            Self::Suspended => "SessionSuspended",
            Self::Expired => "SessionExpired",
        }
    }
}
