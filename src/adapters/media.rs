use std::env;

use async_trait::async_trait;
use tokio::fs;

use crate::error::Result;
use crate::ports::media::MediaFilePort;
use crate::util::hex_encode_id;

pub struct DesktopMediaFiles;

impl DesktopMediaFiles {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl MediaFilePort for DesktopMediaFiles {
    async fn open_media(&self, event_id: &str, data: &[u8]) -> Result<()> {
        let ext = infer::get(data).map_or("bin", |kind| kind.extension());
        let dir = env::temp_dir().join("u2dm-media");
        fs::create_dir_all(&dir).await?;
        let path = dir.join(format!("{}.{ext}", hex_encode_id(event_id)));
        fs::write(&path, data).await?;
        open::that_in_background(&path);
        Ok(())
    }

    async fn save_file(&self, default_filename: &str, data: &[u8]) -> Result<Option<String>> {
        let dialog = rfd::AsyncFileDialog::new().set_file_name(default_filename);
        let Some(file_handle) = dialog.save_file().await else {
            return Ok(None);
        };

        file_handle.write(data).await?;
        Ok(Some(file_handle.path().display().to_string()))
    }
}
