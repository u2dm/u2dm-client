use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use std::{env, fs, process};

use async_trait::async_trait;
use tokio::fs as async_fs;

use crate::adapters::private_fs;
use crate::error::{AppError, Result};
use crate::ports::media::MediaFilePort;
use crate::util::random_hex;

const MEDIA_DIR: &str = "u2dm-media";
const MEDIA_RETENTION: Duration = Duration::from_hours(24);
const SESSION_TOKEN_BYTES: usize = 8;
const FILE_TOKEN_BYTES: usize = 16;
const ROOT_TOKEN_BYTES: usize = 16;
const ROOT_ATTEMPTS: usize = 8;

pub struct DesktopMediaFiles {
    session_dir: Option<PathBuf>,
}

impl DesktopMediaFiles {
    pub fn new() -> Self {
        Self {
            session_dir: open_root().and_then(|root| open_session_dir(&root)),
        }
    }

    fn session_dir(&self) -> Result<&Path> {
        self.session_dir.as_deref().ok_or_else(|| {
            AppError::Other("no private directory is available to open media from".into())
        })
    }
}

#[async_trait]
impl MediaFilePort for DesktopMediaFiles {
    async fn open_media(&self, _event_id: &str, data: &[u8]) -> Result<()> {
        let session_dir = self.session_dir()?;
        let ext = infer::get(data).map_or("bin", |kind| kind.extension());
        private_fs::create_dir(session_dir).await?;
        let path = session_dir.join(format!("{}.{ext}", random_hex(FILE_TOKEN_BYTES)));
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
        let Some(session_dir) = self.session_dir.as_deref() else {
            return;
        };
        match async_fs::remove_dir_all(session_dir).await {
            Ok(()) => tracing::debug!("cleared session media directory"),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("failed to clear session media directory: {e}"),
        }
        if let Err(e) = private_fs::create_dir(session_dir).await {
            tracing::debug!("failed to recreate session media directory: {e}");
        }
    }
}

fn open_root() -> Option<PathBuf> {
    let root = adopt_stable_root().or_else(unpredictable_root)?;
    sweep_stale(&root);
    let temp_root = env::temp_dir().join(MEDIA_DIR);
    if temp_root != root && private_fs::is_private_dir(&temp_root) {
        sweep_stale(&temp_root);
    }
    Some(root)
}

fn adopt_stable_root() -> Option<PathBuf> {
    let stable_root = per_user_dir().unwrap_or_else(env::temp_dir).join(MEDIA_DIR);
    match claim_dir(&stable_root) {
        Claim::Created | Claim::Adopted => Some(stable_root),
        Claim::Rejected => {
            tracing::warn!(
                path = %stable_root.display(),
                "media directory is not private to this user, falling back to an unpredictable one"
            );
            None
        }
    }
}

fn unpredictable_root() -> Option<PathBuf> {
    let parent = env::temp_dir();
    (0..ROOT_ATTEMPTS)
        .map(|_| parent.join(format!("{MEDIA_DIR}-{}", random_hex(ROOT_TOKEN_BYTES))))
        .find(|candidate| matches!(claim_dir(candidate), Claim::Created))
}

fn open_session_dir(root: &Path) -> Option<PathBuf> {
    let session_dir = root.join(format!(
        "session-{}-{}",
        process::id(),
        random_hex(SESSION_TOKEN_BYTES)
    ));
    matches!(claim_dir(&session_dir), Claim::Created).then_some(session_dir)
}

enum Claim {
    Created,
    Adopted,
    Rejected,
}

fn claim_dir(dir: &Path) -> Claim {
    match private_fs::create_dir_exclusive_blocking(dir) {
        Ok(()) => Claim::Created,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            if private_fs::is_private_dir(dir) {
                Claim::Adopted
            } else {
                Claim::Rejected
            }
        }
        Err(e) => {
            tracing::debug!("failed to create media directory {}: {e}", dir.display());
            Claim::Rejected
        }
    }
}

#[cfg(unix)]
fn per_user_dir() -> Option<PathBuf> {
    env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute() && private_fs::is_private_dir(dir))
}

#[cfg(not(unix))]
fn per_user_dir() -> Option<PathBuf> {
    None
}

fn sweep_stale(base_dir: &Path) {
    let Ok(entries) = fs::read_dir(base_dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(metadata) = own_stale_entry(&path, now) else {
            continue;
        };
        remove_entry(&path, metadata.is_dir());
    }
}

fn own_stale_entry(path: &Path, now: SystemTime) -> Option<fs::Metadata> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !private_fs::is_owned_by_current_user(&metadata) {
        return None;
    }
    let age = now.duration_since(metadata.modified().ok()?).ok()?;
    (age > MEDIA_RETENTION).then_some(metadata)
}

fn remove_entry(path: &Path, is_dir: bool) {
    let result = if is_dir {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    if let Err(e) = result {
        tracing::debug!("failed to remove stale media entry {}: {e}", path.display());
    }
}
