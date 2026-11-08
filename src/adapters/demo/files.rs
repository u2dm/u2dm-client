use std::env;
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::{attachments, encryption};
use crate::adapters::media::{DesktopMediaFiles, suggested_save_name};
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

    async fn choose_save_path(
        &self,
        filename: &str,
        mimetype: Option<&str>,
    ) -> Result<Option<PathBuf>> {
        let Some(dir) = attachments::scenario().save_dir.as_deref() else {
            return self.inner.choose_save_path(filename, mimetype).await;
        };
        let path = dir.join(suggested_save_name(filename, mimetype));
        tracing::info!(path = %path.display(), "demo: choosing where to save without a dialog");
        Ok(Some(path))
    }

    async fn save_to(&self, path: &Path, data: Vec<u8>) -> Result<()> {
        self.inner.save_to(path, data).await
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
