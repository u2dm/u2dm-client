use std::sync::Arc;

use super::credentials;
use crate::domain::account::AccountScope;
use crate::domain::auth::Session;
use crate::error::{AppError, Result};
use crate::ports::matrix::{AuthenticatedSession, CleanupReport, StagedCleanup, StoreAdoption};
use crate::ports::storage::{DisplacedCredentials, StoragePort, SupersededLogin};

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

async fn unstage(storage: &dyn StoragePort, txn: &str) -> StagedCleanup {
    match storage.clear_superseded(txn).await {
        Ok(()) => StagedCleanup::Done,
        Err(e) => {
            tracing::warn!("the credentials this login replaced could not be unstaged: {e}");
            StagedCleanup::Pending
        }
    }
}

pub(super) struct EstablishedSession {
    adoption: Box<dyn StoreAdoption>,
    storage: Arc<dyn StoragePort>,
    account: AccountScope,
    displaced: DisplacedCredentials,
}

impl EstablishedSession {
    pub(super) async fn record_or_roll_back(
        adoption: Box<dyn StoreAdoption>,
        storage: Arc<dyn StoragePort>,
        account: AccountScope,
        session: &Session,
        passphrase: &str,
    ) -> Result<Self> {
        let displaced = match credentials::read_displaced(storage.as_ref(), &account).await {
            Ok(displaced) => displaced,
            Err(e) => {
                let report = adoption.roll_back(StagedCleanup::Done).await;
                return Err(also_failed_to_roll_back(e, &report));
            }
        };

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
        let cleanup = unstage(storage.as_ref(), adoption.transaction()).await;
        adoption.commit(cleanup).await
    }

    pub(super) async fn roll_back(self) -> CleanupReport {
        let mut report = CleanupReport::default();

        if let Err(e) = self.adoption.rolling_back().await {
            report.fail(format!(
                "this login could not be marked for rollback, so it is left in place and the previous session is not restored ({e})"
            ));
            return report;
        }

        let restored =
            credentials::restore_displaced(self.storage.as_ref(), &self.account, &self.displaced)
                .await;
        let cleanup = if restored.has_failures() {
            StagedCleanup::Pending
        } else {
            unstage(self.storage.as_ref(), self.adoption.transaction()).await
        };
        report.merge(restored);
        report.merge(self.adoption.roll_back(cleanup).await);
        report
    }

    async fn record(&self, session: &Session, passphrase: &str) -> Result<()> {
        self.stage_displaced().await?;
        self.adoption.credentials_staged().await.map_err(|e| {
            AppError::Other(format!(
                "The credentials this login replaces could not be recorded as staged, so undoing the login after a restart would not know to restore them: {e}"
            ))
        })?;
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
            displaced: self.displaced.clone(),
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
}
