use std::{io, result};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    Ui(String),

    #[error("{0}")]
    Io(#[from] io::Error),
}

pub type Result<T> = result::Result<T, AppError>;
