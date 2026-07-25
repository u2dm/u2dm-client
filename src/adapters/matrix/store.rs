use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tokio::fs;

use crate::domain::account::AccountScope;
use crate::error::{AppError, Result};
use crate::ports::matrix::CleanupReport;
use crate::util::random_hex;

const STORES_DIR: &str = "stores";
const LAYOUT_VERSION: &str = "v1";
const PENDING_PREFIX: &str = "pending-";
const QUARANTINE_PREFIX: &str = "quarantine-";
const ABANDONED_PENDING_AGE: Duration = Duration::from_hours(24);

#[derive(Clone, Debug)]
pub(super) struct StorePaths {
    pub(super) data: PathBuf,
    pub(super) cache: PathBuf,
}

#[derive(Clone)]
pub(super) struct StoreLayout {
    data_dir: PathBuf,
    cache_dir: PathBuf,
}

impl StoreLayout {
    pub(super) fn new(data_dir: PathBuf, cache_dir: PathBuf) -> Self {
        Self {
            data_dir,
            cache_dir,
        }
    }

    pub(super) fn account(&self, account: &AccountScope) -> StorePaths {
        self.child(account.id())
    }

    pub(super) fn pending(&self) -> StorePaths {
        self.child(&format!("{PENDING_PREFIX}{}", random_hex(8)))
    }

    pub(super) async fn adopt(
        &self,
        pending: &StorePaths,
        account: &AccountScope,
    ) -> Result<StorePaths> {
        let target = self.account(account);
        self.discard_superseded_store(&target).await?;

        move_dir(&pending.data, &target.data).await?;
        move_cache_or_start_empty(&pending.cache, &target.cache).await;

        tracing::info!(store = %target.data.display(), "adopted login store for account");
        Ok(target)
    }

    pub(super) async fn purge(&self, paths: &StorePaths) -> CleanupReport {
        let roots = self.roots();
        let mut report = CleanupReport::default();
        purge_dir(&roots.data, &paths.data, &mut report).await;
        purge_dir(&roots.cache, &paths.cache, &mut report).await;
        report
    }

    pub(super) async fn purge_account(&self, account: &AccountScope) -> CleanupReport {
        self.purge(&self.account(account)).await
    }

    pub(super) async fn sweep_stale(&self) {
        let roots = self.roots();
        sweep_root(&roots.data).await;
        sweep_root(&roots.cache).await;
    }

    async fn discard_superseded_store(&self, target: &StorePaths) -> Result<()> {
        let report = self.purge(target).await;
        if report.is_clean() {
            return Ok(());
        }
        Err(AppError::Other(format!(
            "could not clear the previous store for this account: {}",
            report.summary()
        )))
    }

    fn roots(&self) -> StorePaths {
        StorePaths {
            data: self.data_dir.join(STORES_DIR).join(LAYOUT_VERSION),
            cache: self.cache_dir.join(STORES_DIR).join(LAYOUT_VERSION),
        }
    }

    fn child(&self, name: &str) -> StorePaths {
        let roots = self.roots();
        StorePaths {
            data: roots.data.join(name),
            cache: roots.cache.join(name),
        }
    }
}

async fn move_cache_or_start_empty(from: &Path, to: &Path) {
    if !dir_exists(from).await {
        return;
    }
    let Err(e) = move_dir(from, to).await else {
        return;
    };
    tracing::warn!(
        from = %from.display(),
        "could not move the login cache store, starting with an empty one: {e}"
    );
    if let Err(e) = remove_dir(from).await {
        tracing::warn!(path = %from.display(), "could not remove directory: {e}");
    }
}

async fn move_dir(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).await?;
    }
    fs::rename(from, to).await?;
    Ok(())
}

pub(super) async fn purge_dir(root: &Path, dir: &Path, report: &mut CleanupReport) {
    let Err(e) = remove_dir(dir).await else {
        return;
    };
    tracing::warn!(path = %dir.display(), "could not delete store directory: {e}");
    quarantine_dir(root, dir, report).await;
}

async fn quarantine_dir(root: &Path, dir: &Path, report: &mut CleanupReport) {
    if let Err(e) = fs::create_dir_all(root).await {
        report.fail(format!(
            "{} could not be deleted, and {} is unavailable to move it into ({e})",
            dir.display(),
            root.display()
        ));
        return;
    }
    let quarantine = root.join(format!("{QUARANTINE_PREFIX}{}", random_hex(8)));
    if let Err(e) = fs::rename(dir, &quarantine).await {
        report.fail(format!(
            "{} could not be deleted or moved aside ({e})",
            dir.display()
        ));
        return;
    }
    if let Err(e) = remove_dir(&quarantine).await {
        tracing::warn!(path = %quarantine.display(), "quarantined store still on disk: {e}");
        report.quarantined.push(quarantine);
        return;
    }
    tracing::info!(path = %dir.display(), "store directory deleted after being moved aside");
}

async fn remove_dir(dir: &Path) -> Result<()> {
    match fs::remove_dir_all(dir).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

async fn dir_exists(path: &Path) -> bool {
    fs::metadata(path).await.is_ok()
}

async fn sweep_root(root: &Path) {
    let Ok(mut entries) = fs::read_dir(root).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if is_sweepable(&entry).await {
            sweep(&entry.path()).await;
        }
    }
}

async fn is_sweepable(entry: &fs::DirEntry) -> bool {
    let name = entry.file_name();
    let Some(name) = name.to_str() else {
        return false;
    };
    if name.starts_with(QUARANTINE_PREFIX) {
        return true;
    }
    name.starts_with(PENDING_PREFIX) && is_older_than(entry, ABANDONED_PENDING_AGE).await
}

async fn sweep(path: &Path) {
    match remove_dir(path).await {
        Ok(()) => tracing::info!(path = %path.display(), "swept leftover store directory"),
        Err(e) => tracing::warn!(path = %path.display(), "leftover store directory remains: {e}"),
    }
}

async fn is_older_than(entry: &fs::DirEntry, age: Duration) -> bool {
    let Ok(metadata) = entry.metadata().await else {
        return false;
    };
    metadata
        .modified()
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|elapsed| elapsed > age)
}
