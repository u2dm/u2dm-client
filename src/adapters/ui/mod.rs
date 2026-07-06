#![allow(clippy::pub_use)]

mod common;
mod emoji;
mod output;

use slint::PlatformError;

use crate::error::AppError;

pub use output::UiEventOutput;

impl From<PlatformError> for AppError {
    fn from(err: PlatformError) -> Self {
        Self::Ui(err.to_string())
    }
}

#[cfg(not(feature = "interpreted"))]
mod compiled;
#[cfg(not(feature = "interpreted"))]
pub use compiled::SlintUiAdapter;

#[cfg(feature = "interpreted")]
mod interpreted;
#[cfg(feature = "interpreted")]
pub use interpreted::SlintUiAdapter;
