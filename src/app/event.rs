use std::path::PathBuf;

use super::establish::EstablishedSession;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::LoginActivity;
use crate::domain::models::{ServerInfo, VerificationEvent};
use crate::ports::matrix::{AuthenticatedSession, CleanupReport};

#[derive(Clone, Copy)]
pub(super) enum EndReason {
    UserLogout,
    Expired,
}

pub(super) enum AppEvent {
    Session(SessionEvent),
    VerificationFlow(VerificationEvent),
    VerificationActionFailed(UserMessageKind),
}

impl AppEvent {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Session(event) => event.label(),
            Self::VerificationFlow(_) => "VerificationFlow",
            Self::VerificationActionFailed(_) => "VerificationActionFailed",
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
        }
    }
}
