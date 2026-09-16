use crate::domain::account::AccountScope;
use crate::error::{AppError, Result};
use crate::ports::matrix::CleanupReport;
use crate::ports::storage::{DisplacedCredentials, StoragePort, StoredSession};

fn unreadable_displaced(what: &str, err: &AppError) -> AppError {
    AppError::Other(format!(
        "The {what} this login would replace could not be read, so the login was not started. \
         Undoing it later would have destroyed the previous session's local data: {err}"
    ))
}

pub(super) async fn read_displaced(
    storage: &dyn StoragePort,
    account: &AccountScope,
) -> Result<DisplacedCredentials> {
    let session = match storage.load_session().await {
        Ok(StoredSession::Present(session)) => Some(session),
        Ok(StoredSession::Absent | StoredSession::Incomplete) => None,
        Ok(StoredSession::CredentialsUnavailable(e)) | Err(e) => {
            return Err(unreadable_displaced("session", &e));
        }
    };

    let passphrase = storage
        .load_passphrase(account)
        .await
        .map_err(|e| unreadable_displaced("local store key", &e))?;

    Ok(DisplacedCredentials {
        session,
        passphrase,
    })
}

pub(super) async fn restore_displaced(
    storage: &dyn StoragePort,
    account: &AccountScope,
    displaced: &DisplacedCredentials,
) -> CleanupReport {
    let mut report = CleanupReport::default();

    let restored_session = match &displaced.session {
        Some(session) => storage.save_session(session).await,
        None => storage.clear_session().await,
    };
    if let Err(e) = restored_session {
        report.fail(format!("the previous session could not be put back ({e})"));
    }

    let restored_key = match &displaced.passphrase {
        Some(passphrase) => storage.save_passphrase(account, passphrase).await,
        None => storage.clear_passphrase(account).await,
    };
    if let Err(e) = restored_key {
        report.fail(format!(
            "the previous local store key could not be put back ({e})"
        ));
    }

    report
}
