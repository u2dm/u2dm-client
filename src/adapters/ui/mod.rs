#![allow(clippy::pub_use)]

mod autolink;
mod backend;
mod clock;
mod decode;
mod dto;
#[cfg(feature = "demo")]
pub(crate) mod dump;
mod emoji;
mod multiplex;
mod output;
mod present;
mod props;
mod reconcile;
mod reduce;
mod richtext;
mod router;
mod rows;
pub(crate) mod schema;
mod video;

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
