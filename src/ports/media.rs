use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::domain::media::{AttachmentPick, ContentKey, MediaFailure, PickedAttachment, Waveform};
use crate::error::Result;

#[async_trait]
pub trait MediaFilePort: Send + Sync {
    async fn open_media(&self, event_id: &str, data: &[u8]) -> Result<()>;
    async fn open_path(&self, path: &Path) -> Result<()>;
    async fn pick_attachment(&self, pick: AttachmentPick) -> Result<Option<PickedAttachment>>;
    async fn release_attachment(&self, picked: PickedAttachment);
    async fn save_file(&self, default_filename: &str, data: &[u8]) -> Result<Option<String>>;
    async fn clear_session(&self);
}

pub trait MediaCache: Send + Sync {
    fn thumbnail_path(&self, content: &ContentKey) -> Option<PathBuf>;
    fn thumbnail_failure(&self, content: &ContentKey) -> Option<MediaFailure>;
    fn user_avatar_path(&self, mxc: &str) -> Option<PathBuf>;
    fn room_avatar_path(&self, mxc: &str) -> Option<PathBuf>;
    fn space_avatar_path(&self, mxc: &str) -> Option<PathBuf>;
    fn sticker_path(&self, mxc: &str) -> Option<PathBuf>;
    fn sticker_failed(&self, mxc: &str) -> bool;
    fn audio_path(&self, content: &ContentKey) -> Option<PathBuf>;
    fn audio_failure(&self, content: &ContentKey) -> Option<MediaFailure>;
    fn audio_waveform(&self, content: &ContentKey) -> Option<Waveform>;
}
