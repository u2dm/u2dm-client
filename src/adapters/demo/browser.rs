use async_trait::async_trait;

use crate::error::Result;
use crate::ports::browser::BrowserPort;

pub struct DemoBrowser;

#[async_trait]
impl BrowserPort for DemoBrowser {
    async fn open_url(&self, url: &str) -> Result<()> {
        tracing::info!(url, "demo mode: not opening a real browser");
        Ok(())
    }
}
