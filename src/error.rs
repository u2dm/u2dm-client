use std::{fmt, io, result};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("UI: {0}")]
    Ui(String),

    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Matrix(#[from] matrix_sdk::Error),

    #[error("Keyring ({key}): {source}")]
    Keyring {
        key: String,
        source: keyring_core::Error,
    },

    #[error(transparent)]
    Serde(#[from] serde_json::Error),

    #[error("Configuration: {0}")]
    Config(String),

    #[error("authentication failed ({kind}): {detail}")]
    Auth { kind: AuthFailure, detail: String },

    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFailure {
    Unreachable,
    InvalidCredentials,
    AccountDeactivated,
    InvalidUsername,
    RateLimited,
    MethodUnsupported,
    IdentityDiverged,
    Unknown,
}

impl fmt::Display for AuthFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Unreachable => "unreachable",
            Self::InvalidCredentials => "invalid credentials",
            Self::AccountDeactivated => "account deactivated",
            Self::InvalidUsername => "invalid username",
            Self::RateLimited => "rate limited",
            Self::MethodUnsupported => "method unsupported",
            Self::IdentityDiverged => "local signing key diverged from the published one",
            Self::Unknown => "unknown",
        };
        f.write_str(name)
    }
}

pub type Result<T> = result::Result<T, AppError>;
