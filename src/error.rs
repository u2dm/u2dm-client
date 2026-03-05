use std::{io, result};

use matrix_sdk::ruma::api::client::error::ErrorKind as RumaErrorKind;
use thiserror::Error;

use crate::domain::models::UiErrorKind;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("UI: {0}")]
    Ui(String),

    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Matrix(#[from] matrix_sdk::Error),

    #[error("Keyring ({key}): {source}")]
    Keyring { key: String, source: keyring::Error },

    #[error(transparent)]
    Serde(#[from] serde_json::Error),

    #[error("Session expired")]
    SessionExpired,

    #[error("Configuration: {0}")]
    Config(String),

    #[error("{0}")]
    Other(String),
}

impl AppError {
    pub fn ui_error_kind(&self) -> UiErrorKind {
        match self {
            Self::Matrix(e) => classify_matrix_error(e),
            Self::Io(_) | Self::Keyring { .. } | Self::Serde(_) => UiErrorKind::Storage,
            Self::SessionExpired => UiErrorKind::Authentication,
            Self::Config(_) | Self::Ui(_) | Self::Other(_) => UiErrorKind::Other,
        }
    }
}

fn classify_matrix_error(err: &matrix_sdk::Error) -> UiErrorKind {
    if matches!(
        err.client_api_error_kind(),
        Some(
            RumaErrorKind::Unauthorized
                | RumaErrorKind::Forbidden { .. }
                | RumaErrorKind::UnknownToken { .. }
        )
    ) {
        return UiErrorKind::Authentication;
    }
    if matches!(err, matrix_sdk::Error::Http(_)) && err.client_api_error_kind().is_none() {
        return UiErrorKind::Network;
    }
    UiErrorKind::Other
}

pub type Result<T> = result::Result<T, AppError>;
