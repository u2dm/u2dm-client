use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use tokio::fs;

use crate::adapters::private_fs;
use crate::domain::account::AccountScope;
use crate::error::{AppError, Result};
use crate::ports::matrix::{LoginResolution, PendingLogin};
use crate::util::random_hex;

const JOURNAL_PREFIX: &str = "txn-";
const JOURNAL_SUFFIX: &str = ".json";
const JOURNAL_VERSION: u8 = 1;
const TXN_ID_BYTES: usize = 8;

pub(super) const BACKUP_PREFIX: &str = "backup-";

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum LoginStage {
    Prepared,
    OldStoreHeldAside,
    NewStoreInstalled,
    CredentialsWritten,
    Committed,
}

impl LoginStage {
    fn resolution(self) -> LoginResolution {
        match self {
            Self::Prepared | Self::OldStoreHeldAside | Self::NewStoreInstalled => {
                LoginResolution::RollBack
            }
            Self::CredentialsWritten | Self::Committed => LoginResolution::RollForward,
        }
    }

    fn installed_store_is_ours(self) -> bool {
        matches!(
            self,
            Self::OldStoreHeldAside | Self::NewStoreInstalled | Self::CredentialsWritten
        )
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct JournalRecord {
    version: u8,
    txn: String,
    account: String,
    stage: LoginStage,
    installing: String,
}

pub(super) struct LoginJournal {
    path: PathBuf,
    record: JournalRecord,
}

impl LoginJournal {
    pub(super) async fn open(
        root: &Path,
        account: &AccountScope,
        installing: &str,
    ) -> Result<Self> {
        private_fs::create_dir(root).await?;
        let txn = random_hex(TXN_ID_BYTES);
        let journal = Self {
            path: journal_path(root, &txn),
            record: JournalRecord {
                version: JOURNAL_VERSION,
                txn,
                account: account.id().to_owned(),
                stage: LoginStage::Prepared,
                installing: installing.to_owned(),
            },
        };
        journal.persist().await?;
        tracing::info!(
            txn = %journal.record.txn,
            account = %journal.record.account,
            "opened the login transaction journal"
        );
        Ok(journal)
    }

    pub(super) async fn load(root: &Path, txn: &str) -> Result<Self> {
        let path = journal_path(root, txn);
        let raw = fs::read(&path).await?;
        let record: JournalRecord = serde_json::from_slice(&raw)?;
        if record.version != JOURNAL_VERSION {
            return Err(AppError::Other(format!(
                "the login journal at {} uses an unsupported layout (version {})",
                path.display(),
                record.version
            )));
        }
        if record.txn != txn {
            return Err(AppError::Other(format!(
                "the login journal at {} names transaction {} instead",
                path.display(),
                record.txn
            )));
        }
        Ok(Self { path, record })
    }

    pub(super) fn txn(&self) -> &str {
        &self.record.txn
    }

    pub(super) fn stage(&self) -> LoginStage {
        self.record.stage
    }

    pub(super) fn installing(&self) -> &str {
        &self.record.installing
    }

    pub(super) fn displaced(&self) -> String {
        format!("{BACKUP_PREFIX}{}", self.record.txn)
    }

    pub(super) fn account(&self) -> AccountScope {
        AccountScope::from_id(self.record.account.clone())
    }

    pub(super) fn pending_login(&self) -> PendingLogin {
        PendingLogin {
            txn: self.record.txn.clone(),
            account: self.account(),
            resolution: self.record.stage.resolution(),
        }
    }

    pub(super) fn protected_names(&self) -> [String; 2] {
        [self.record.installing.clone(), self.displaced()]
    }

    pub(super) fn installed_store_is_ours(&self) -> bool {
        self.record.stage.installed_store_is_ours()
    }

    pub(super) async fn advance(&mut self, stage: LoginStage) -> Result<()> {
        if self.record.stage == stage {
            return Ok(());
        }
        let previous = self.record.stage;
        self.record.stage = stage;
        if let Err(e) = self.persist().await {
            self.record.stage = previous;
            return Err(e);
        }
        tracing::debug!(txn = %self.record.txn, ?stage, "login transaction advanced");
        Ok(())
    }

    pub(super) async fn discard(self) {
        match fs::remove_file(&self.path).await {
            Ok(()) => self.report_closed().await,
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                path = %self.path.display(),
                "the login journal could not be removed, it will be recovered again: {e}"
            ),
        }
    }

    async fn report_closed(&self) {
        if let Err(e) = private_fs::sync_containing_dir(&self.path).await {
            tracing::debug!("could not sync the journal directory after removal: {e}");
        }
        tracing::info!(txn = %self.record.txn, "login transaction closed");
    }

    async fn persist(&self) -> Result<()> {
        let encoded = serde_json::to_vec(&self.record)?;
        private_fs::write_durably(&self.path, &encoded).await?;
        Ok(())
    }
}

pub(super) async fn load_all(root: &Path) -> Vec<LoginJournal> {
    let Ok(mut entries) = fs::read_dir(root).await else {
        return Vec::new();
    };
    let mut journals = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Some(journal) = load_entry(root, &entry.file_name()).await {
            journals.push(journal);
        }
    }
    journals
}

async fn load_entry(root: &Path, file_name: &OsStr) -> Option<LoginJournal> {
    let txn = file_name.to_str().and_then(txn_of)?;
    match LoginJournal::load(root, txn).await {
        Ok(journal) => Some(journal),
        Err(e) => {
            tracing::warn!(txn, "an unreadable login journal was left in place: {e}");
            None
        }
    }
}

fn journal_path(root: &Path, txn: &str) -> PathBuf {
    root.join(format!("{JOURNAL_PREFIX}{txn}{JOURNAL_SUFFIX}"))
}

fn txn_of(file_name: &str) -> Option<&str> {
    file_name
        .strip_prefix(JOURNAL_PREFIX)?
        .strip_suffix(JOURNAL_SUFFIX)
        .filter(|txn| !txn.is_empty())
}
