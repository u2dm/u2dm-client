use std::result;
use std::sync::Arc;

use super::conclude::{self, Closing};
use super::credentials;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::domain::account::AccountScope;
use crate::domain::auth::Session;
use crate::error::{AppError, Result};
use crate::ports::matrix::{AuthPort, AuthenticatedSession, CleanupReport, StoreAdoption};
use crate::ports::storage::{DisplacedCredentials, StoragePort, SupersededLogin};

pub(super) type Establishing<T> = result::Result<T, LoginFailure>;

pub(super) enum LoginFailure {
    Rejected(AppError),
    Unresolved(UserMessage),
}

impl From<AppError> for LoginFailure {
    fn from(err: AppError) -> Self {
        Self::Rejected(err)
    }
}

pub(super) enum Rollback {
    Complete(CleanupReport),
    Unresolved(UserMessage),
}

impl Rollback {
    fn unresolved(detail: &str) -> Self {
        Self::Unresolved(UserMessage::about(
            UserMessageKind::InterruptedLoginUnresolved,
            &detail,
        ))
    }

    fn into_failure(self, cause: AppError) -> LoginFailure {
        match self {
            Self::Complete(report) => {
                LoginFailure::Rejected(also_failed_to_roll_back(cause, &report))
            }
            Self::Unresolved(message) => {
                tracing::error!("the login that could not be undone had failed with: {cause}");
                LoginFailure::Unresolved(message)
            }
        }
    }
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

async fn finish_rollback(
    adoption: Box<dyn StoreAdoption>,
    auth: &dyn AuthPort,
    storage: &dyn StoragePort,
    mut report: CleanupReport,
) -> Rollback {
    let txn = adoption.transaction().to_owned();
    report.merge(adoption.unwind().await);
    if report.has_failures() {
        return Rollback::unresolved(&format!(
            "login {txn} could not be undone, so its journal and the credentials it staged are \
             kept for the next start ({})",
            report.summary()
        ));
    }
    match conclude::close_rolled_back(auth, storage, &txn).await {
        Ok(Closing::Closed | Closing::CleanupPending) => Rollback::Complete(report),
        Err(e) => Rollback::unresolved(&format!(
            "login {txn} was undone, but that could not be recorded, so its staged credentials \
             are still needed ({e})"
        )),
    }
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
    ) -> Establishing<Self> {
        let displaced = match credentials::read_displaced(storage.as_ref(), &account).await {
            Ok(displaced) => displaced,
            Err(e) => {
                let rollback = finish_rollback(
                    adoption,
                    auth.as_ref(),
                    storage.as_ref(),
                    CleanupReport::default(),
                )
                .await;
                return Err(rollback.into_failure(e));
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
                let rollback = established.roll_back().await;
                Err(rollback.into_failure(e))
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

    pub(super) async fn roll_back(self) -> Rollback {
        let Self {
            adoption,
            auth,
            storage,
            account,
            displaced,
        } = self;

        if let Err(e) = adoption.rolling_back().await {
            return Rollback::unresolved(&format!(
                "login {} could not be marked for rollback, so it is left in place and the \
                 previous session is not restored ({e})",
                adoption.transaction()
            ));
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
