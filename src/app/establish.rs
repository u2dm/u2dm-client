use std::sync::Arc;

use crate::domain::account::AccountScope;
use crate::domain::models::Session;
use crate::error::{AppError, Result};
use crate::ports::matrix::{AuthenticatedSession, CleanupReport, StoreAdoption};
use crate::ports::storage::{StoragePort, StoredSession, SupersededLogin};

struct DisplacedRecords {
    session: Option<Session>,
    passphrase: Option<String>,
}

fn also_failed_to_roll_back(err: AppError, report: &CleanupReport) -> AppError {
    if report.is_clean() {
        return err;
    }
    let detail = report.summary();
    tracing::warn!("the previous session was not fully restored: {detail}");
    AppError::Other(format!(
        "{err} The previous session could not be fully restored either: {detail}"
    ))
}

impl DisplacedRecords {
    async fn read(storage: &dyn StoragePort, account: &AccountScope) -> Self {
        let session = match storage.load_session().await {
            Ok(StoredSession::Present(session)) => Some(session),
            _ => None,
        };
        Self {
            session,
            passphrase: storage.load_passphrase(account).await.ok().flatten(),
        }
    }
}

pub(super) struct EstablishedSession {
    adoption: Box<dyn StoreAdoption>,
    storage: Arc<dyn StoragePort>,
    account: AccountScope,
    displaced: DisplacedRecords,
}

impl EstablishedSession {
    pub(super) async fn record_or_roll_back(
        adoption: Box<dyn StoreAdoption>,
        storage: Arc<dyn StoragePort>,
        account: AccountScope,
        session: &Session,
        passphrase: &str,
    ) -> Result<Self> {
        let displaced = DisplacedRecords::read(storage.as_ref(), &account).await;
        let established = Self {
            adoption,
            storage,
            account,
            displaced,
        };

        match established.record(session, passphrase).await {
            Ok(()) => Ok(established),
            Err(e) => {
                let report = established.roll_back().await;
                Err(also_failed_to_roll_back(e, &report))
            }
        }
    }

    pub(super) async fn commit(self) -> AuthenticatedSession {
        let Self {
            adoption, storage, ..
        } = self;
        if let Err(e) = storage.clear_superseded().await {
            tracing::warn!("the credentials this login replaced could not be unstaged: {e}");
        }
        adoption.commit().await
    }

    pub(super) async fn roll_back(self) -> CleanupReport {
        let mut report = CleanupReport::default();

        if let Err(e) = self.adoption.rolling_back().await {
            report.fail(format!(
                "this login could not be marked for rollback, so it is left in place and the previous session is not restored ({e})"
            ));
            return report;
        }

        report.merge(self.restore_displaced().await);
        report.merge(self.adoption.roll_back().await);
        if let Err(e) = self.storage.clear_superseded().await {
            tracing::warn!("the restored credentials could not be unstaged: {e}");
        }
        report
    }

    async fn record(&self, session: &Session, passphrase: &str) -> Result<()> {
        self.stage_displaced().await?;
        self.write_records(session, passphrase).await?;
        self.adoption.credentials_written().await.map_err(|e| {
            AppError::Other(format!(
                "The login could not be recorded as complete, so it would not survive a restart: {e}"
            ))
        })
    }

    async fn stage_displaced(&self) -> Result<()> {
        let superseded = SupersededLogin {
            txn: self.adoption.transaction().to_owned(),
            session: self.displaced.session.clone(),
            passphrase: self.displaced.passphrase.clone(),
        };
        self.storage.save_superseded(&superseded).await.map_err(|e| {
            AppError::Other(format!(
                "The credentials this login replaces could not be staged, so the login was not started: {e}"
            ))
        })
    }

    async fn write_records(&self, session: &Session, passphrase: &str) -> Result<()> {
        self.storage
            .save_passphrase(&self.account, passphrase)
            .await
            .map_err(|e| {
                AppError::Other(format!(
                    "The key to the local store could not be saved, so the session would not survive a restart: {e}"
                ))
            })?;

        self.storage.save_session(session).await.map_err(|e| {
            AppError::Other(format!(
                "The session could not be saved, so it would not survive a restart: {e}"
            ))
        })
    }

    async fn restore_displaced(&self) -> CleanupReport {
        let mut report = CleanupReport::default();

        let restored_session = match &self.displaced.session {
            Some(session) => self.storage.save_session(session).await,
            None => self.storage.clear_session().await,
        };
        if let Err(e) = restored_session {
            report.fail(format!("the previous session could not be put back ({e})"));
        }

        let restored_key = match &self.displaced.passphrase {
            Some(passphrase) => {
                self.storage
                    .save_passphrase(&self.account, passphrase)
                    .await
            }
            None => self.storage.clear_passphrase(&self.account).await,
        };
        if let Err(e) = restored_key {
            report.fail(format!(
                "the previous local store key could not be put back ({e})"
            ));
        }

        report
    }
}
