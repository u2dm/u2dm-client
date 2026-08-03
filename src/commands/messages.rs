use std::fmt;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct UserMessage {
    pub kind: UserMessageKind,
    pub detail: String,
}

impl UserMessage {
    pub fn new(kind: UserMessageKind) -> Self {
        Self {
            kind,
            detail: String::new(),
        }
    }

    pub fn about(kind: UserMessageKind, detail: &impl fmt::Display) -> Self {
        Self {
            kind,
            detail: detail.to_string(),
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum UserMessageKind {
    #[default]
    None,
    ServerUnreachable,
    UnsupportedLoginMethod,
    LoginFailed,
    InvalidCredentials,
    AccountDeactivated,
    InvalidUsername,
    RateLimited,
    LoginMethodUnsupported,
    SessionUnreadable,
    SessionRestoreFailed,
    StoreKeyMissing,
    StoreKeyUnreadable,
    IdentityDiverged,
    SessionExpired,
    DataQuarantined,
    DataNotErased,
    InterruptedLoginUnresolved,
    SessionSaveFailed,
    SendMessageFailed,
    LoadMoreFailed,
    MessageNotFound,
    MessageNotShowable,
    SpaceOrderSaveFailed,
    MediaDownloadFailed,
    FileDownloadFailed,
    MediaOpenFailed,
    MediaNotViewable,
    FileSaveFailed,
    FileSaved,
    VerificationAcceptFailed,
    VerificationConfirmFailed,
    VerificationRejectFailed,
    VerificationTimedOut,
    VerificationSasAcceptFailed,
    VerificationCancelled,
    VerificationDeclined,
    VerificationMismatch,
    VerificationAcceptedElsewhere,
    VerificationFailed,
}
