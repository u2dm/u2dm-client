use crate::ports::matrix::{AuthPort, CleanupReport, LoginResolution, PendingLogin};
use crate::ports::storage::StoragePort;

pub(super) async fn recover_interrupted_logins(auth: &dyn AuthPort, storage: &dyn StoragePort) {
    let interrupted = auth.pending_logins().await;
    if interrupted.is_empty() {
        return;
    }
    tracing::info!(
        count = interrupted.len(),
        "resolving interrupted logins before anything else touches the stores"
    );
    for login in &interrupted {
        resolve(auth, storage, login).await;
    }
}

async fn resolve(auth: &dyn AuthPort, storage: &dyn StoragePort, login: &PendingLogin) {
    let report = match login.resolution {
        LoginResolution::RollBack => roll_back(auth, storage, login).await,
        LoginResolution::RollForward => auth.settle_login(&login.txn).await,
    };
    if report.has_failures() {
        report_unresolved(login, &report);
    } else {
        close(auth, storage, login).await;
    }
}

fn report_unresolved(login: &PendingLogin, report: &CleanupReport) {
    tracing::warn!(
        txn = %login.txn,
        "an interrupted login could not be resolved and will be retried at the next start: {}",
        report.summary()
    );
}

async fn close(auth: &dyn AuthPort, storage: &dyn StoragePort, login: &PendingLogin) {
    if let Err(e) = storage.clear_superseded().await {
        tracing::warn!("the staged credentials could not be unstaged: {e}");
    }
    auth.forget_login(&login.txn).await;
    tracing::info!(txn = %login.txn, ?login.resolution, "interrupted login resolved");
}

async fn roll_back(
    auth: &dyn AuthPort,
    storage: &dyn StoragePort,
    login: &PendingLogin,
) -> CleanupReport {
    let mut report = auth.unwind_login(&login.txn).await;
    if report.has_failures() {
        return report;
    }
    report.merge(restore_credentials(storage, login).await);
    report
}

async fn restore_credentials(storage: &dyn StoragePort, login: &PendingLogin) -> CleanupReport {
    let mut report = CleanupReport::default();

    let staged = match storage.load_superseded().await {
        Ok(Some(staged)) if staged.txn == login.txn => staged,
        Ok(Some(staged)) => {
            tracing::warn!(
                txn = %login.txn,
                staged = %staged.txn,
                "the staged credentials belong to another login, leaving the credential store as it is"
            );
            return report;
        }
        Ok(None) => {
            tracing::info!(
                txn = %login.txn,
                "no credentials were staged for this login, leaving the credential store as it is"
            );
            return report;
        }
        Err(e) => {
            report.fail(format!(
                "the credentials this login replaced could not be read back ({e})"
            ));
            return report;
        }
    };

    let restored_session = match &staged.session {
        Some(session) => storage.save_session(session).await,
        None => storage.clear_session().await,
    };
    if let Err(e) = restored_session {
        report.fail(format!("the previous session could not be put back ({e})"));
    }

    let restored_key = match &staged.passphrase {
        Some(passphrase) => storage.save_passphrase(&login.account, passphrase).await,
        None => storage.clear_passphrase(&login.account).await,
    };
    if let Err(e) = restored_key {
        report.fail(format!(
            "the previous local store key could not be put back ({e})"
        ));
    }

    report
}
