use std::path::PathBuf;

use crate::commands::messages::UserMessage;
use crate::domain::auth::LoginMethod;
use crate::domain::capabilities::AccountFacts;
use crate::domain::encryption::{KeyStorage, Recovery, RecoveryKey, Standing};
use crate::domain::sessions::{LogoutRisks, OwnSessions};
use crate::domain::sync::ReauthReason;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct EncryptionView {
    pub standing: Standing,
    pub verified_sessions: bool,
    pub recovery: Recovery,
    pub key_storage: KeyStorage,
    pub dialog: Option<EncryptionDialog>,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct EncryptionDialog {
    pub step: EncryptionStep,
    pub activity: EncryptionActivity,
    pub error: UserMessage,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub enum EncryptionStep {
    #[default]
    Overview,
    EnteringKey,
    Unlocked,
    UnlockedWithoutBackup,
    ShowingKey(RecoveryKey),
    ConfirmingReset,
    AuthenticatingSetup(AuthPrompt),
    AuthenticatingReset(AuthPrompt),
    IdentityReset,
    ExportingKeys(String),
    KeysExported {
        count: usize,
        file: String,
    },
    ImportingKeys(String),
    KeysImported {
        imported: usize,
        total: usize,
    },
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum EncryptionActivity {
    #[default]
    None,
    Requesting,
    Unlocking,
    SettingUpRecovery,
    ChangingKey,
    RetryingSetup,
    ResettingIdentity,
    Authenticating,
    ChoosingFile,
    ExportingKeys,
    ImportingKeys,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum AuthStage {
    #[default]
    Password,
    Approve,
    Browser,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct AuthPrompt {
    pub stage: AuthStage,
    pub error: UserMessage,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct AccountView {
    pub dialog: Option<AccountDialog>,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct AccountDialog {
    pub step: AccountStep,
    pub activity: AccountActivity,
    pub busy_session: Option<String>,
    pub sessions: Option<OwnSessions>,
    pub facts: Option<AccountFacts>,
    pub error: UserMessage,
    pub notice: UserMessage,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub enum AccountStep {
    #[default]
    Overview,
    ChangingPassword,
    ConfirmingDeactivation,
    Authenticating(AccountAuth),
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct AccountAuth {
    pub prompt: AuthPrompt,
    pub guarding: AccountActivity,
    pub subject: String,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum AccountActivity {
    #[default]
    None,
    Renaming,
    SigningOut,
    ChangingPassword,
    Deactivating,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct LogoutView {
    pub warning: Option<LogoutRisks>,
}

#[derive(Clone, Default)]
pub struct LifecycleView {
    pub step: LoginStep,
    pub activity: LoginActivity,
    pub messages: Vec<UserMessage>,
    pub method: LoginMethod,
    pub resolved_homeserver: String,
    pub user_id: String,
    pub device_id: String,
    pub avatar_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum LoginStep {
    #[default]
    Loading,
    RestorePaused,
    Homeserver,
    Credentials,
    Reauthenticate(ReauthReason),
    LoggedIn,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum LoginActivity {
    #[default]
    Idle,
    LoadingSession,
    OpeningStore,
    Connecting,
    RestoringAuth,
    CheckingServer,
    LoggingIn,
    OpeningBrowser,
    WaitingAuth,
    Cancelling,
    Syncing,
    SigningOut,
    CleaningUp,
}
