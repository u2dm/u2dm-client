use std::fmt;

use crate::domain::identity::SessionAlert;
use crate::domain::uia::AuthChallenge;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionTrust {
    #[default]
    Unknown,
    NoIdentity,
    Unverified,
    Verified,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Recovery {
    #[default]
    Unknown,
    Off,
    On,
    Incomplete,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KeyStorage {
    #[default]
    Allowed,
    TurnedOff,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AccountEncryption {
    pub trust: SessionTrust,
    pub verified_sessions: bool,
    pub recovery: Recovery,
    pub key_storage: KeyStorage,
}

impl AccountEncryption {
    pub fn recovery_unlocks(self) -> bool {
        matches!(self.recovery, Recovery::On | Recovery::Incomplete)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IdentitySetup {
    #[default]
    Running,
    Settled,
    NeedsAuthentication,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityOutcome {
    Ready,
    NeedsAuthentication(Box<AuthChallenge>),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Standing {
    #[default]
    Checking,
    SetupRefused,
    SetupFailed,
    Unverified,
    Verified,
}

impl Standing {
    pub fn of(trust: SessionTrust, setup: IdentitySetup) -> Self {
        match (trust, setup) {
            (SessionTrust::Verified, _) => Self::Verified,
            (SessionTrust::Unverified, _) => Self::Unverified,
            (SessionTrust::NoIdentity, IdentitySetup::NeedsAuthentication) => Self::SetupRefused,
            (SessionTrust::NoIdentity, IdentitySetup::Failed) => Self::SetupFailed,
            (SessionTrust::NoIdentity, IdentitySetup::Running | IdentitySetup::Settled)
            | (SessionTrust::Unknown, _) => Self::Checking,
        }
    }

    pub fn alert(self) -> Option<SessionAlert> {
        match self {
            Self::Unverified => Some(SessionAlert::Unverified),
            Self::SetupRefused | Self::SetupFailed => Some(SessionAlert::NotSetUp),
            Self::Checking | Self::Verified => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RecoveryKey(String);

impl RecoveryKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryKey([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAttempt {
    Unlocked,
    UnlockedWithoutBackup,
    WrongKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoverySetup {
    Created(RecoveryKey),
    BackupInTheWay,
}

#[derive(Clone, PartialEq, Eq)]
pub struct KeyPassphrase(String);

impl KeyPassphrase {
    pub fn new(passphrase: impl Into<String>) -> Self {
        Self(passphrase.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for KeyPassphrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KeyPassphrase([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyImport {
    Imported { imported: usize, total: usize },
    WrongPassphrase,
    NotAKeyFile,
}
