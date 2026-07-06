use async_trait::async_trait;

use crate::error::Result;

#[async_trait]
pub trait MediaFilePort: Send + Sync {
    async fn open_media(&self, event_id: &str, data: &[u8]) -> Result<()>;
    async fn save_file(&self, default_filename: &str, data: &[u8]) -> Result<Option<String>>;
}
