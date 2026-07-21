use std::path::PathBuf;

use async_trait::async_trait;

use crate::error::Result;

#[async_trait]
pub trait MediaFilePort: Send + Sync {
    async fn open_media(&self, event_id: &str, data: &[u8]) -> Result<()>;
    async fn save_file(&self, default_filename: &str, data: &[u8]) -> Result<Option<String>>;
    async fn clear_session(&self);
}

pub trait MediaCache: Send + Sync {
    fn thumbnail_path(&self, event_id: &str) -> Option<PathBuf>;
    fn thumbnail_failed(&self, event_id: &str) -> bool;
    fn avatar_path(&self, sender: &str) -> Option<PathBuf>;
    fn room_avatar_path(&self, mxc: &str) -> Option<PathBuf>;
    fn space_avatar_path(&self, mxc: &str) -> Option<PathBuf>;
}
