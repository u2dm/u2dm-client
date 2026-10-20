use std::env;
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::{attachments, encryption};
use crate::adapters::media::DesktopMediaFiles;
use crate::domain::media::{AttachmentPick, PickedAttachment};
use crate::error::Result;
use crate::ports::media::MediaFilePort;

const DEMO_KEY_FILE: &str = "demo-room-keys.txt";

fn chosen_key_file(name: &str) -> Option<PathBuf> {
    if encryption::scenario().key_file_cancelled {
        tracing::info!("demo: the key file chooser was cancelled");
        return None;
    }
    let path = env::temp_dir().join(name);
    tracing::info!(path = %path.display(), "demo: choosing a key file without a dialog");
    Some(path)
}

pub struct DemoMediaFiles {
    inner: DesktopMediaFiles,
}

impl DemoMediaFiles {
    pub fn new() -> Self {
        Self {
            inner: DesktopMediaFiles::new(),
        }
    }
}

#[async_trait]
impl MediaFilePort for DemoMediaFiles {
    async fn open_media(&self, event_id: &str, data: &[u8]) -> Result<()> {
        self.inner.open_media(event_id, data).await
    }

    async fn open_path(&self, path: &Path) -> Result<()> {
        self.inner.open_path(path).await
    }

    async fn pick_attachment(&self, pick: AttachmentPick) -> Result<Option<PickedAttachment>> {
        match (pick, attachments::scenario().preset_pick.as_deref()) {
            (AttachmentPick::Media | AttachmentPick::Document, Some(preset)) => {
                tracing::info!(path = %preset.display(), "demo: picking a preset attachment");
                self.inner.describe(preset).await.map(Some)
            }
            (pick, _) => self.inner.pick_attachment(pick).await,
        }
    }

    async fn release_attachment(&self, _picked: PickedAttachment) {}

    async fn save_file(&self, default_filename: &str, data: &[u8]) -> Result<Option<String>> {
        self.inner.save_file(default_filename, data).await
    }

    async fn choose_key_export(&self, default_filename: &str) -> Result<Option<PathBuf>> {
        Ok(chosen_key_file(default_filename))
    }

    async fn pick_key_file(&self) -> Result<Option<PathBuf>> {
        Ok(chosen_key_file(DEMO_KEY_FILE))
    }

    async fn clear_session(&self) {
        self.inner.clear_session().await;
    }
}
