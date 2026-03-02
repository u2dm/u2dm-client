use std::{io, result};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    Ui(String),

    #[error("{0}")]
    Io(#[from] io::Error),

    #[error("{0}")]
    Matrix(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = result::Result<T, AppError>;
