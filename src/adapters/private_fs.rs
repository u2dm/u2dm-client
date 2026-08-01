use std::ffi::OsString;
use std::fs as std_fs;
use std::io::{self, ErrorKind};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use rustix::process::geteuid;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::util::random_hex;

#[cfg(unix)]
const OWNER_ONLY_DIR: u32 = 0o700;
#[cfg(unix)]
const OWNER_ONLY_FILE: u32 = 0o600;
#[cfg(unix)]
const EXPOSED_TO_OTHERS: u32 = 0o077;
const TMP_TOKEN_BYTES: usize = 8;

pub(crate) async fn create_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(OWNER_ONLY_DIR);
    builder.create(dir).await
}

pub(crate) fn create_dir_exclusive_blocking(dir: &Path) -> io::Result<()> {
    let mut builder = std_fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(OWNER_ONLY_DIR);
    builder.create(dir)
}

pub(crate) fn is_private_dir(path: &Path) -> bool {
    std_fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_dir() && is_owned_by_current_user(&metadata) && !is_exposed(&metadata)
    })
}

#[cfg(unix)]
pub(crate) fn is_owned_by_current_user(metadata: &std_fs::Metadata) -> bool {
    metadata.uid() == geteuid().as_raw()
}

#[cfg(not(unix))]
pub(crate) fn is_owned_by_current_user(_metadata: &std_fs::Metadata) -> bool {
    true
}

#[cfg(unix)]
fn is_exposed(metadata: &std_fs::Metadata) -> bool {
    metadata.permissions().mode() & EXPOSED_TO_OTHERS != 0
}

#[cfg(not(unix))]
fn is_exposed(_metadata: &std_fs::Metadata) -> bool {
    false
}

pub(crate) async fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    write_owner_only(path, data, Durability::Buffered).await
}

pub(crate) async fn write_atomically(path: &Path, data: &[u8]) -> io::Result<()> {
    stage_and_rename(path, data, Durability::Buffered).await
}

pub(crate) async fn write_durably(path: &Path, data: &[u8]) -> io::Result<()> {
    stage_and_rename(path, data, Durability::Synced).await?;
    sync_containing_dir(path).await
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Durability {
    Buffered,
    Synced,
}

async fn write_owner_only(path: &Path, data: &[u8], durability: Durability) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(OWNER_ONLY_FILE);
    let mut file = options.open(path).await?;
    file.write_all(data).await?;
    file.flush().await?;
    if durability == Durability::Synced {
        file.sync_all().await?;
    }
    Ok(())
}

async fn stage_and_rename(path: &Path, data: &[u8], durability: Durability) -> io::Result<()> {
    let tmp = unique_tmp_path(path);
    if let Err(e) = write_owner_only(&tmp, data, durability).await {
        discard_temp(&tmp).await;
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, path).await {
        discard_temp(&tmp).await;
        return Err(e);
    }
    Ok(())
}

pub(crate) async fn sync_containing_dir(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(parent) => sync_dir(parent).await,
        None => Ok(()),
    }
}

#[cfg(unix)]
pub(crate) async fn sync_dir(dir: &Path) -> io::Result<()> {
    fs::File::open(dir).await?.sync_all().await
}

#[cfg(not(unix))]
pub(crate) async fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

async fn discard_temp(tmp: &Path) {
    if let Err(e) = fs::remove_file(tmp).await
        && e.kind() != ErrorKind::NotFound
    {
        tracing::debug!("failed to remove staged temp file {}: {e}", tmp.display());
    }
}

fn unique_tmp_path(path: &Path) -> PathBuf {
    let mut name = match path.file_name() {
        Some(file_name) => file_name.to_os_string(),
        None => OsString::from("tmp"),
    };
    name.push(format!(".{}.tmp", random_hex(TMP_TOKEN_BYTES)));
    path.with_file_name(name)
}

#[cfg(unix)]
pub(crate) async fn restrict_existing(root: &Path) {
    let mut dirs_to_walk = vec![root.to_path_buf()];
    let mut repaired = 0_usize;
    while let Some(dir) = dirs_to_walk.pop() {
        repaired =
            repaired.saturating_add(usize::from(restrict_if_exposed(&dir, OWNER_ONLY_DIR).await));
        repaired = repaired.saturating_add(restrict_children(&dir, &mut dirs_to_walk).await);
    }
    if repaired > 0 {
        tracing::info!(
            path = %root.display(),
            "restricted {repaired} pre-existing entries to the current user"
        );
    }
}

#[cfg(unix)]
async fn restrict_children(dir: &Path, dirs_to_walk: &mut Vec<PathBuf>) -> usize {
    let Ok(mut entries) = fs::read_dir(dir).await else {
        return 0;
    };
    let mut repaired = 0_usize;
    while let Ok(Some(entry)) = entries.next_entry().await {
        match entry.file_type().await {
            Ok(file_type) if file_type.is_dir() => dirs_to_walk.push(entry.path()),
            Ok(file_type) if file_type.is_file() => {
                let restricted = restrict_if_exposed(&entry.path(), OWNER_ONLY_FILE).await;
                repaired = repaired.saturating_add(usize::from(restricted));
            }
            _ => {}
        }
    }
    repaired
}

#[cfg(unix)]
async fn restrict_if_exposed(path: &Path, owner_only: u32) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path).await else {
        return false;
    };
    if metadata.permissions().mode() & EXPOSED_TO_OTHERS == 0 {
        return false;
    }
    match fs::set_permissions(path, std_fs::Permissions::from_mode(owner_only)).await {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!("failed to restrict {}: {e}", path.display());
            false
        }
    }
}

#[cfg(not(unix))]
pub(crate) async fn restrict_existing(_root: &Path) {}
