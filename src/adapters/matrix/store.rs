use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tokio::fs;
use tokio::sync::Mutex;

use super::journal::{self, BACKUP_PREFIX, LoginJournal, LoginStage};
use crate::adapters::private_fs;
use crate::domain::account::AccountScope;
use crate::error::{AppError, Result};
use crate::ports::matrix::{CleanupReport, PendingLogin, StagedCleanup};
use crate::util::random_hex;

const STORES_DIR: &str = "stores";
const LAYOUT_VERSION: &str = "v1";
const PENDING_PREFIX: &str = "pending-";
const QUARANTINE_PREFIX: &str = "quarantine-";
const ABANDONED_STORE_AGE: Duration = Duration::from_hours(24);

#[derive(Clone, Debug)]
pub(super) struct StorePaths {
    pub(super) name: String,
    pub(super) data: PathBuf,
    pub(super) cache: PathBuf,
}

struct Roots {
    data: PathBuf,
    cache: PathBuf,
}

pub(super) struct AdoptedStore {
    pub(super) paths: StorePaths,
    pub(super) txn: String,
    journal: Mutex<LoginJournal>,
}

impl AdoptedStore {
    pub(super) async fn credentials_staged(&self) -> Result<()> {
        self.journal.lock().await.mark_credentials_staged().await
    }

    pub(super) async fn credentials_written(&self) -> Result<()> {
        self.journal
            .lock()
            .await
            .advance(LoginStage::CredentialsWritten)
            .await
    }

    pub(super) async fn rolling_back(&self) -> Result<()> {
        let mut journal = self.journal.lock().await;
        if journal.stage() != LoginStage::CredentialsWritten {
            return Ok(());
        }
        journal.advance(LoginStage::NewStoreInstalled).await
    }
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
        let mut journal = LoginJournal::open(&self.roots().data, account, &pending.name).await?;
        let target = self.account(account);

        if let Err(e) = self.install_journaled(&mut journal, pending, &target).await {
            let report = self.unwind(&mut journal).await;
            if !report.is_clean() {
                tracing::warn!(
                    "previous store not restored after a failed adoption: {}",
                    report.summary()
                );
            }
            journal.discard().await;
            return Err(e);
        }

        tracing::info!(store = %target.data.display(), "adopted login store for account");
        Ok(AdoptedStore {
            paths: target,
            txn: journal.txn().to_owned(),
            journal: Mutex::new(journal),
        })
    }

    pub(super) async fn commit_adoption(&self, adopted: AdoptedStore, cleanup: StagedCleanup) {
        let mut journal = adopted.journal.into_inner();
        let report = self.settle(&mut journal).await;
        close_or_retry(journal, cleanup).await;
        if report.is_clean() {
            tracing::info!("previous store for this account discarded after adoption");
        } else {
            tracing::warn!(
                "previous store not fully removed after adoption: {}",
                report.summary()
            );
        }
    }

    pub(super) async fn roll_back_adoption(
        &self,
        adopted: AdoptedStore,
        cleanup: StagedCleanup,
    ) -> CleanupReport {
        let mut journal = adopted.journal.into_inner();
        let report = self.unwind(&mut journal).await;
        close_or_retry(journal, cleanup).await;
        report
    }

    pub(super) async fn pending_logins(&self) -> Vec<PendingLogin> {
        journal::load_all(&self.roots().data)
            .await
            .iter()
            .map(LoginJournal::pending_login)
            .collect()
    }

    pub(super) async fn unwind_login(&self, txn: &str) -> CleanupReport {
        match LoginJournal::load(&self.roots().data, txn).await {
            Ok(mut journal) => self.unwind(&mut journal).await,
            Err(e) => unreadable_journal(txn, &e),
        }
    }

    pub(super) async fn settle_login(&self, txn: &str) -> CleanupReport {
        match LoginJournal::load(&self.roots().data, txn).await {
            Ok(mut journal) => self.settle(&mut journal).await,
            Err(e) => unreadable_journal(txn, &e),
        }
    }

    pub(super) async fn forget_login(&self, txn: &str) {
        match LoginJournal::load(&self.roots().data, txn).await {
            Ok(journal) => journal.discard().await,
            Err(e) => tracing::warn!(txn, "the login journal could not be closed: {e}"),
        }
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
        let protected = self.protected_names().await;
        sweep_root(&roots.data, &protected).await;
        sweep_root(&roots.cache, &protected).await;
    }

    async fn install_journaled(
        &self,
        journal: &mut LoginJournal,
        pending: &StorePaths,
        target: &StorePaths,
    ) -> Result<()> {
        hold_aside(target, &self.child(&journal.displaced())).await?;
        journal.advance(LoginStage::OldStoreHeldAside).await?;
        install(pending, target).await?;
        journal.advance(LoginStage::NewStoreInstalled).await
    }

    async fn unwind(&self, journal: &mut LoginJournal) -> CleanupReport {
        let target = self.account(&journal.account());
        let aside = self.child(&journal.displaced());
        let mut report = CleanupReport::default();

        if journal.installed_store_is_ours() {
            report.merge(self.purge(&target).await);
            if let Err(e) = journal.advance(LoginStage::Prepared).await {
                report.fail(format!(
                    "the login transaction could not be marked as unwound, so the previous store is left at {} ({e})",
                    aside.data.display()
                ));
                return report;
            }
        }

        match put_back(&aside, &target).await {
            Ok(()) => tracing::info!("previous store for this account put back"),
            Err(e) => report.fail(format!(
                "the previous store for this account is held at {} and could not be put back ({e})",
                aside.data.display()
            )),
        }

        report.merge(self.purge(&self.child(journal.installing())).await);
        report
    }

    async fn settle(&self, journal: &mut LoginJournal) -> CleanupReport {
        if journal.stage() != LoginStage::CredentialsWritten {
            return CleanupReport::default();
        }
        let mut report = self.purge(&self.child(&journal.displaced())).await;
        if let Err(e) = journal.advance(LoginStage::Committed).await {
            report.fail(format!(
                "the login transaction could not be marked as committed ({e})"
            ));
        }
        report
    }

    async fn protected_names(&self) -> HashSet<String> {
        journal::load_all(&self.roots().data)
            .await
            .iter()
            .flat_map(LoginJournal::protected_names)
            .collect()
    }

    fn roots(&self) -> Roots {
        Roots {
            data: self.data_dir.join(STORES_DIR).join(LAYOUT_VERSION),
            cache: self.cache_dir.join(STORES_DIR).join(LAYOUT_VERSION),
        }
    }

    fn child(&self, name: &str) -> StorePaths {
        let roots = self.roots();
        StorePaths {
            name: name.to_owned(),
            data: roots.data.join(name),
            cache: roots.cache.join(name),
        }
    }
}

async fn close_or_retry(journal: LoginJournal, cleanup: StagedCleanup) {
    match cleanup {
        StagedCleanup::Done => journal.discard().await,
        StagedCleanup::Pending => tracing::warn!(
            txn = journal.txn(),
            "the credentials this login replaced are still staged, so the journal is kept for the next start"
        ),
    }
}

fn unreadable_journal(txn: &str, error: &AppError) -> CleanupReport {
    let mut report = CleanupReport::default();
    report.fail(format!(
        "the interrupted login {txn} could not be resolved because its journal is unreadable ({error})"
    ));
    report
}

async fn install(pending: &StorePaths, target: &StorePaths) -> Result<()> {
    move_dir(&pending.data, &target.data).await?;
    move_cache_or_start_empty(&pending.cache, &target.cache).await;
    Ok(())
}

async fn hold_aside(target: &StorePaths, aside: &StorePaths) -> Result<()> {
    move_existing(&target.data, &aside.data).await?;
    move_existing(&target.cache, &aside.cache).await?;
    Ok(())
}

async fn put_back(aside: &StorePaths, target: &StorePaths) -> Result<()> {
    move_existing(&aside.data, &target.data).await?;
    move_existing(&aside.cache, &target.cache).await?;
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
    if let Some(parent) = to.parent()
        && let Err(e) = private_fs::sync_dir(parent).await
    {
        tracing::debug!(path = %parent.display(), "could not sync the store directory: {e}");
    }
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

async fn sweep_root(root: &Path, protected: &HashSet<String>) {
    let Ok(mut entries) = fs::read_dir(root).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if is_sweepable(&entry, protected).await {
            sweep(&entry.path()).await;
        }
    }
}

async fn is_sweepable(entry: &fs::DirEntry, protected: &HashSet<String>) -> bool {
    let name = entry.file_name();
    let Some(name) = name.to_str() else {
        return false;
    };
    if protected.contains(name) {
        tracing::debug!(name, "leaving a store an interrupted login still needs");
        return false;
    }
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
