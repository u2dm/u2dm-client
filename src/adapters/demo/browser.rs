use async_trait::async_trait;

use crate::domain::link::LauncherSafeUrl;
use crate::error::Result;
use crate::ports::browser::BrowserPort;

pub struct DemoBrowser;

#[async_trait]
impl BrowserPort for DemoBrowser {
    async fn open_url(&self, url: &LauncherSafeUrl) -> Result<()> {
        tracing::info!(url = url.as_str(), "demo mode: not opening a real browser");
        Ok(())
    }
}
