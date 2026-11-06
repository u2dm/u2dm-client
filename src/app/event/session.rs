use std::path::PathBuf;

use super::EndReason;
use crate::app::establish::EstablishedSession;
use crate::commands::messages::UserMessage;
use crate::commands::view::LoginActivity;
use crate::domain::auth::ServerInfo;
use crate::domain::sync::{ReauthReason, SessionLoss};
use crate::ports::matrix::{AuthenticatedSession, CleanupReport};

pub(in crate::app) enum SessionEvent {
    RestoreProgress(LoginActivity),
    RestoreFailed(Vec<UserMessage>),
    RestorePaused(UserMessage),
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
        messages: Vec<UserMessage>,
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
        left_behind: Option<String>,
        report: CleanupReport,
    },
    TokensNotPersisted {
        session: u64,
    },
    UserAvatar {
        session: u64,
        path: Option<PathBuf>,
    },
    Suspended(ReauthReason),
    Expired,
}

impl From<SessionLoss> for SessionEvent {
    fn from(loss: SessionLoss) -> Self {
        match loss {
            SessionLoss::Suspended(reason) => Self::Suspended(reason),
            SessionLoss::Expired => Self::Expired,
        }
    }
}

impl SessionEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::RestoreProgress(_) => "RestoreProgress",
            Self::RestoreFailed(_) => "RestoreFailed",
            Self::RestorePaused(_) => "RestorePaused",
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
            Self::TokensNotPersisted { .. } => "TokensNotPersisted",
            Self::UserAvatar { .. } => "UserAvatar",
            Self::Suspended(_) => "SessionSuspended",
            Self::Expired => "SessionExpired",
        }
    }
}
