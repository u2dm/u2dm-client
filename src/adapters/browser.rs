use async_trait::async_trait;
use tokio::task::spawn_blocking;

use crate::domain::link::LauncherSafeUrl;
use crate::error::{AppError, Result};
use crate::ports::browser::BrowserPort;

pub struct DesktopBrowser;

impl DesktopBrowser {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl BrowserPort for DesktopBrowser {
    async fn open_url(&self, url: &LauncherSafeUrl) -> Result<()> {
        let url = url.as_str().to_owned();
        spawn_blocking(move || open::that_detached(&url))
            .await
            .map_err(|e| AppError::Other(format!("failed to launch browser: {e}")))??;
        Ok(())
    }
}
