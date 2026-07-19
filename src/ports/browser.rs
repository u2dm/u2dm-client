use async_trait::async_trait;

use crate::error::Result;

#[async_trait]
pub trait BrowserPort: Send + Sync {
    async fn open_url(&self, url: &str) -> Result<()>;
}
