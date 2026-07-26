use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use std::{env, fs, process};

use async_trait::async_trait;
use tokio::fs as async_fs;

use crate::adapters::private_fs;
use crate::error::Result;
use crate::ports::media::MediaFilePort;
use crate::util::random_hex;

const MEDIA_DIR: &str = "u2dm-media";
const MEDIA_RETENTION: Duration = Duration::from_hours(24);
const SESSION_TOKEN_BYTES: usize = 8;
const FILE_TOKEN_BYTES: usize = 16;

pub struct DesktopMediaFiles {
    session_dir: PathBuf,
}

impl DesktopMediaFiles {
    pub fn new() -> Self {
        let base_dir = env::temp_dir().join(MEDIA_DIR);
        prepare_dir_blocking(&base_dir);
        sweep_stale(&base_dir);
        let session_dir = base_dir.join(format!(
            "session-{}-{}",
            process::id(),
            random_hex(SESSION_TOKEN_BYTES)
        ));
        prepare_dir_blocking(&session_dir);
        Self { session_dir }
    }
}

#[async_trait]
impl MediaFilePort for DesktopMediaFiles {
    async fn open_media(&self, _event_id: &str, data: &[u8]) -> Result<()> {
        let ext = infer::get(data).map_or("bin", |kind| kind.extension());
        private_fs::create_dir(&self.session_dir).await?;
        let path = self
            .session_dir
            .join(format!("{}.{ext}", random_hex(FILE_TOKEN_BYTES)));
        private_fs::write_private(&path, data).await?;
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

    async fn clear_session(&self) {
        match async_fs::remove_dir_all(&self.session_dir).await {
            Ok(()) => tracing::debug!("cleared session media directory"),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("failed to clear session media directory: {e}"),
        }
        if let Err(e) = private_fs::create_dir(&self.session_dir).await {
            tracing::debug!("failed to recreate session media directory: {e}");
        }
    }
}

fn prepare_dir_blocking(dir: &Path) {
    if let Err(e) = private_fs::create_dir_blocking(dir) {
        tracing::debug!("failed to create media directory {}: {e}", dir.display());
    }
}

fn sweep_stale(base_dir: &Path) {
    let Ok(entries) = fs::read_dir(base_dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        if is_stale(&entry, now) {
            remove_entry(&entry.path());
        }
    }
}

fn is_stale(entry: &fs::DirEntry, now: SystemTime) -> bool {
    let Ok(metadata) = entry.metadata() else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    now.duration_since(modified)
        .is_ok_and(|age| age > MEDIA_RETENTION)
}

fn remove_entry(path: &Path) {
    let result = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    if let Err(e) = result {
        tracing::debug!("failed to remove stale media entry {}: {e}", path.display());
    }
}
