use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tokio::fs;

use crate::adapters::private_fs;
use crate::domain::account::AccountScope;
use crate::error::{AppError, Result};
use crate::ports::matrix::CleanupReport;
use crate::util::random_hex;

const STORES_DIR: &str = "stores";
const LAYOUT_VERSION: &str = "v1";
const PENDING_PREFIX: &str = "pending-";
const BACKUP_PREFIX: &str = "backup-";
const QUARANTINE_PREFIX: &str = "quarantine-";
const ABANDONED_STORE_AGE: Duration = Duration::from_hours(24);

#[derive(Clone, Debug)]
pub(super) struct StorePaths {
    pub(super) data: PathBuf,
    pub(super) cache: PathBuf,
}

pub(super) struct AdoptedStore {
    pub(super) paths: StorePaths,
    held_aside: Option<StorePaths>,
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
    ) -> Result<AdoptedStore> {
        let target = self.account(account);
        let held_aside = self.hold_aside(&target).await?;
        let adopted = AdoptedStore {
            paths: target,
            held_aside,
        };

        if let Err(e) = install(pending, &adopted.paths).await {
            let report = self.roll_back_adoption(adopted).await;
            if !report.is_clean() {
                tracing::warn!(
                    "previous store not restored after a failed adoption: {}",
                    report.summary()
                );
            }
            return Err(e);
        }

        tracing::info!(store = %adopted.paths.data.display(), "adopted login store for account");
        Ok(adopted)
    }

    pub(super) async fn commit_adoption(&self, adopted: AdoptedStore) {
        let Some(held_aside) = adopted.held_aside else {
            return;
        };
        let report = self.purge(&held_aside).await;
        if report.is_clean() {
            tracing::info!("previous store for this account discarded after adoption");
        } else {
            tracing::warn!(
                "previous store not fully removed after adoption: {}",
                report.summary()
            );
        }
    }

    pub(super) async fn roll_back_adoption(&self, adopted: AdoptedStore) -> CleanupReport {
        let mut report = self.purge(&adopted.paths).await;
        let Some(held_aside) = adopted.held_aside else {
            return report;
        };
        match put_back(&held_aside, &adopted.paths).await {
            Ok(()) => tracing::info!("previous store for this account put back"),
            Err(e) => report.fail(format!(
                "the previous store for this account is held at {} and could not be put back ({e})",
                held_aside.data.display()
            )),
        }
        report
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

    async fn hold_aside(&self, target: &StorePaths) -> Result<Option<StorePaths>> {
        let aside = self.child(&format!("{BACKUP_PREFIX}{}", random_hex(8)));
        let data_held = move_existing(&target.data, &aside.data).await?;

        match move_existing(&target.cache, &aside.cache).await {
            Ok(cache_held) => Ok((data_held || cache_held).then_some(aside)),
            Err(e) if !data_held => Err(e),
            Err(e) => match move_dir(&aside.data, &target.data).await {
                Ok(()) => Err(e),
                Err(undo) => Err(AppError::Other(format!(
                    "the previous store for this account could not be held aside ({e}), and it is now at {} instead of {} ({undo})",
                    aside.data.display(),
                    target.data.display()
                ))),
            },
        }
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

async fn install(pending: &StorePaths, target: &StorePaths) -> Result<()> {
    move_dir(&pending.data, &target.data).await?;
    move_cache_or_start_empty(&pending.cache, &target.cache).await;
    Ok(())
}

async fn put_back(held_aside: &StorePaths, target: &StorePaths) -> Result<()> {
    move_existing(&held_aside.data, &target.data).await?;
    move_existing(&held_aside.cache, &target.cache).await?;
    Ok(())
}

async fn move_existing(from: &Path, to: &Path) -> Result<bool> {
    if !dir_exists(from).await {
        return Ok(false);
    }
    move_dir(from, to).await?;
    Ok(true)
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
        private_fs::create_dir(parent).await?;
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
    if let Err(e) = private_fs::create_dir(root).await {
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
    let abandonable = name.starts_with(PENDING_PREFIX) || name.starts_with(BACKUP_PREFIX);
    abandonable && is_older_than(entry, ABANDONED_STORE_AGE).await
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
