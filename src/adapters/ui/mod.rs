#![allow(clippy::pub_use)]

mod backend;
mod decode;
mod dto;
mod emoji;
mod multiplex;
mod output;
mod present;
mod props;
mod reconcile;
mod reduce;
mod router;
mod schema;

pub use output::UiEventOutput;
use slint::PlatformError;

use crate::error::AppError;

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
