use std::path::PathBuf;

use super::establish::EstablishedSession;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::LoginActivity;
use crate::domain::auth::ServerInfo;
use crate::domain::media::PickedAttachment;
use crate::domain::room::RoomId;
use crate::domain::timeline::{
    PaginationDirection, PaginationOutcome, TimelineAdvance, TimelineFocus,
};
use crate::domain::verification::VerificationEvent;
use crate::ports::matrix::{AuthenticatedSession, CleanupReport};

#[derive(Clone, Copy)]
pub(super) enum EndReason {
    UserLogout,
    Expired,
}

pub(super) struct AttachmentPicked {
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
        room_id: RoomId,
        failure: Option<UserMessage>,
    },
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
}

impl TimelineEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::Advanced { .. } => "TimelineAdvanced",
            Self::PaginationCompleted { .. } => "TimelinePaginationCompleted",
            Self::Refocus { .. } => "RefocusTimeline",
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
            Self::ErasingLocalState { .. } => "ErasingLocalState",
            Self::LocalStateCleared { .. } => "LocalStateCleared",
            Self::TokensNotPersisted => "TokensNotPersisted",
            Self::UserAvatar(_) => "UserAvatar",
            Self::Expired => "SessionExpired",
        }
    }
}
