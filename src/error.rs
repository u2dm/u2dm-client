use std::{io, result};

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

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = result::Result<T, AppError>;
