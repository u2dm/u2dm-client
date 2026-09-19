use std::sync::Arc;

use super::{conclude, credentials};
use crate::domain::account::AccountScope;
use crate::domain::auth::Session;
use crate::error::{AppError, Result};
use crate::ports::matrix::{AuthPort, AuthenticatedSession, CleanupReport, StoreAdoption};
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

async fn finish_rollback(
    adoption: Box<dyn StoreAdoption>,
    auth: &dyn AuthPort,
    storage: &dyn StoragePort,
    mut report: CleanupReport,
) -> CleanupReport {
    let txn = adoption.transaction().to_owned();
    report.merge(adoption.unwind().await);
    if report.has_failures() {
        tracing::warn!(
            txn,
            "the login was not fully undone, so its journal and staged credentials are kept for the next start"
        );
        return report;
    }
    if let Err(e) = conclude::close_rolled_back(auth, storage, &txn).await {
        tracing::warn!(
            txn,
            "the undone login could not be recorded as rolled back, so the next start repeats the rollback: {e}"
        );
    }
    report
}

pub(super) struct EstablishedSession {
    adoption: Box<dyn StoreAdoption>,
    auth: Arc<dyn AuthPort>,
    storage: Arc<dyn StoragePort>,
    account: AccountScope,
    displaced: DisplacedCredentials,
}

impl EstablishedSession {
    pub(super) async fn record_or_roll_back(
        adoption: Box<dyn StoreAdoption>,
        auth: Arc<dyn AuthPort>,
        storage: Arc<dyn StoragePort>,
        account: AccountScope,
        session: &Session,
        passphrase: &str,
    ) -> Result<Self> {
        let displaced = match credentials::read_displaced(storage.as_ref(), &account).await {
            Ok(displaced) => displaced,
            Err(e) => {
                let report = finish_rollback(
                    adoption,
                    auth.as_ref(),
                    storage.as_ref(),
                    CleanupReport::default(),
                )
                .await;
                return Err(also_failed_to_roll_back(e, &report));
            }
        };

        let established = Self {
            adoption,
            auth,
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
            adoption,
            auth,
            storage,
            ..
        } = self;
        let txn = adoption.transaction().to_owned();
        let authenticated = adoption.commit().await;
        conclude::close_terminal(auth.as_ref(), storage.as_ref(), &txn).await;
        authenticated
    }

    pub(super) async fn roll_back(self) -> CleanupReport {
        let Self {
            adoption,
            auth,
            storage,
            account,
            displaced,
        } = self;

        if let Err(e) = adoption.rolling_back().await {
            let mut report = CleanupReport::default();
            report.fail(format!(
                "this login could not be marked for rollback, so it is left in place and the previous session is not restored ({e})"
            ));
            return report;
        }

        let restored = credentials::restore_displaced(storage.as_ref(), &account, &displaced).await;
        finish_rollback(adoption, auth.as_ref(), storage.as_ref(), restored).await
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
