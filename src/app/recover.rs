use crate::ports::matrix::{AuthPort, CleanupReport, LoginResolution, PendingLogin};
use crate::ports::storage::{StagedCredentials, StoragePort, SupersededLogin};

pub(super) enum Recovery {
    Clean,
    Blocked(String),
}

enum Outcome {
    Resolved,
    Retry,
    Blocked(String),
}

enum CredentialPlan {
    Restore(SupersededLogin),
    NothingStaged,
}

pub(super) async fn recover_interrupted_logins(
    auth: &dyn AuthPort,
    storage: &dyn StoragePort,
) -> Recovery {
    let interrupted = auth.pending_logins().await;
    if interrupted.is_empty() {
        return Recovery::Clean;
    }
    tracing::info!(
        count = interrupted.len(),
        "resolving interrupted logins before anything else touches the stores"
    );

    let mut blocked = Vec::new();
    for login in &interrupted {
        match resolve(auth, storage, login).await {
            Outcome::Resolved => {}
            Outcome::Retry => tracing::warn!(
                txn = %login.txn,
                "an interrupted login was left for the next start"
            ),
            Outcome::Blocked(reason) => blocked.push(reason),
        }
    }

    if blocked.is_empty() {
        Recovery::Clean
    } else {
        Recovery::Blocked(blocked.join("; "))
    }
}

async fn resolve(auth: &dyn AuthPort, storage: &dyn StoragePort, login: &PendingLogin) -> Outcome {
    match login.resolution {
        LoginResolution::RollBack => roll_back(auth, storage, login).await,
        LoginResolution::RollForward => roll_forward(auth, storage, login).await,
    }
}

async fn roll_forward(
    auth: &dyn AuthPort,
    storage: &dyn StoragePort,
    login: &PendingLogin,
) -> Outcome {
    let report = auth.settle_login(&login.txn).await;
    if report.has_failures() {
        report_unresolved(login, &report);
        return Outcome::Retry;
    }
    close(auth, storage, login).await
}

async fn roll_back(
    auth: &dyn AuthPort,
    storage: &dyn StoragePort,
    login: &PendingLogin,
) -> Outcome {
    let plan = match credential_plan(storage, login).await {
        Ok(plan) => plan,
        Err(reason) => {
            tracing::error!(
                txn = %login.txn,
                "refusing to undo an interrupted login: {reason}"
            );
            return Outcome::Blocked(reason);
        }
    };

    let mut report = auth.unwind_login(&login.txn).await;
    if report.has_failures() {
        report_unresolved(login, &report);
        return Outcome::Blocked(report.summary());
    }

    report.merge(apply(storage, login, &plan).await);
    if report.has_failures() {
        report_unresolved(login, &report);
        return Outcome::Blocked(report.summary());
    }

    close(auth, storage, login).await
}

async fn credential_plan(
    storage: &dyn StoragePort,
    login: &PendingLogin,
) -> Result<CredentialPlan, String> {
    let staged = storage.load_superseded().await.map_err(|e| {
        format!(
            "the credentials login {} replaced could not be read back ({e})",
            login.txn
        )
    })?;

    match staged {
        StagedCredentials::Present(staged) if staged.txn == login.txn => {
            Ok(CredentialPlan::Restore(staged))
        }
        StagedCredentials::Present(staged) if !login.credentials_staged => {
            tracing::info!(
                txn = %login.txn,
                staged = %staged.txn,
                "the staged credentials belong to another login, and this one staged none"
            );
            Ok(CredentialPlan::NothingStaged)
        }
        StagedCredentials::Present(staged) => Err(format!(
            "login {} staged the previous session's credentials, but the credential store now holds \
             login {}'s instead, so they cannot be restored",
            login.txn, staged.txn
        )),
        StagedCredentials::Corrupt if !login.credentials_staged => {
            tracing::warn!(
                txn = %login.txn,
                "unreadable staged credentials belong to no known login, and this one staged none"
            );
            Ok(CredentialPlan::NothingStaged)
        }
        StagedCredentials::Corrupt => Err(format!(
            "login {} staged the previous session's credentials, but they are unreadable, so they \
             cannot be restored",
            login.txn
        )),
        StagedCredentials::Absent if !login.credentials_staged => Ok(CredentialPlan::NothingStaged),
        StagedCredentials::Absent => Err(format!(
            "login {} staged the previous session's credentials, but they are gone, so they cannot \
             be restored",
            login.txn
        )),
    }
}

async fn apply(
    storage: &dyn StoragePort,
    login: &PendingLogin,
    plan: &CredentialPlan,
) -> CleanupReport {
    let mut report = CleanupReport::default();
    let staged = match plan {
        CredentialPlan::Restore(staged) => staged,
        CredentialPlan::NothingStaged => {
            tracing::info!(
                txn = %login.txn,
                "no credentials were staged for this login, leaving the credential store as it is"
            );
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

fn report_unresolved(login: &PendingLogin, report: &CleanupReport) {
    tracing::warn!(
        txn = %login.txn,
        "an interrupted login could not be resolved and will be retried at the next start: {}",
        report.summary()
    );
}

async fn close(auth: &dyn AuthPort, storage: &dyn StoragePort, login: &PendingLogin) -> Outcome {
    if let Err(e) = storage.clear_superseded(&login.txn).await {
        tracing::warn!(
            txn = %login.txn,
            "the staged credentials could not be unstaged, so the login stays open: {e}"
        );
        return Outcome::Retry;
    }
    auth.forget_login(&login.txn).await;
    tracing::info!(txn = %login.txn, ?login.resolution, "interrupted login resolved");
    Outcome::Resolved
}
