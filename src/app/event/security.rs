use crate::app::account::AccountOutcome;
use crate::app::encryption::EncryptionOutcome;
use crate::commands::messages::UserMessage;
use crate::domain::capabilities::AccountFacts;
use crate::domain::encryption::{AccountEncryption, IdentityOutcome};
use crate::domain::identity::{OwnIdentity, RoomAlert};
use crate::domain::sessions::{LogoutRisks, OwnSessions};

pub(in crate::app) enum EncryptionEvent {
    Changed {
        session: u64,
        state: AccountEncryption,
    },
    IdentitySettled {
        session: u64,
        outcome: Option<IdentityOutcome>,
    },
    ActionSettled {
        request: u64,
        outcome: EncryptionOutcome,
    },
}

impl EncryptionEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::Changed { .. } => "EncryptionChanged",
            Self::IdentitySettled { .. } => "IdentitySetupSettled",
            Self::ActionSettled { .. } => "EncryptionActionSettled",
        }
    }
}

pub(in crate::app) enum AccountEvent {
    FactsRead {
        generation: u64,
        facts: AccountFacts,
    },
    SessionsChanged {
        generation: u64,
        sessions: OwnSessions,
    },
    Settled {
        request: u64,
        outcome: AccountOutcome,
    },
    LogoutChecked {
        request: u64,
        risks: LogoutRisks,
    },
}

impl AccountEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::FactsRead { .. } => "AccountFactsRead",
            Self::SessionsChanged { .. } => "OwnSessionsChanged",
            Self::Settled { .. } => "AccountActionSettled",
            Self::LogoutChecked { .. } => "LogoutChecked",
        }
    }
}

pub(in crate::app) enum IdentityEvent {
    RoomAlerts {
        watch: u64,
        alerts: Vec<RoomAlert>,
    },
    OwnIdentity {
        session: u64,
        own: OwnIdentity,
    },
    Settled {
        request: u64,
        failure: Option<UserMessage>,
    },
}

impl IdentityEvent {
    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::RoomAlerts { .. } => "IdentityAlertsChanged",
            Self::OwnIdentity { .. } => "OwnIdentityChanged",
            Self::Settled { .. } => "IdentityResolutionSettled",
        }
    }
}
