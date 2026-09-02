use async_trait::async_trait;

use super::attachments;
use crate::adapters::media::{self, DesktopMediaFiles};
use crate::domain::media::{AttachmentPick, PickedAttachment};
use crate::error::Result;
use crate::ports::media::MediaFilePort;

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

    async fn pick_attachment(&self, pick: AttachmentPick) -> Result<Option<PickedAttachment>> {
        let Some(preset) = attachments::scenario().preset_pick.as_deref() else {
            return self.inner.pick_attachment(pick).await;
        };
        tracing::info!(path = %preset.display(), "demo: picking a preset attachment");
        media::describe(preset).await.map(Some)
    }

    async fn save_file(&self, default_filename: &str, data: &[u8]) -> Result<Option<String>> {
        self.inner.save_file(default_filename, data).await
    }

    async fn clear_session(&self) {
        self.inner.clear_session().await;
    }
}
