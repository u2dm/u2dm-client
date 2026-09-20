use super::conclude::{self, Closing};
use super::credentials;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::error::AppError;
use crate::ports::matrix::{
    AuthPort, CleanupReport, InterruptedLogin, LocalDataOwnership, LoginResolution, PendingLogin,
};
use crate::ports::storage::{StagedCredentials, StoragePort, SupersededLogin};

pub(super) enum Recovery {
    Clean,
    Blocked(UserMessage),
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
    match auth.local_data_ownership() {
        LocalDataOwnership::Exclusive => {}
        LocalDataOwnership::AnotherInstance { lock } => return another_instance(&lock),
        LocalDataOwnership::Undetermined { reason } => return unclaimed(&reason),
    }

    let interrupted = match auth.interrupted_logins().await {
        Ok(interrupted) => interrupted,
        Err(e) => return unlisted(&e),
    };
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
                txn = login.txn(),
                "an interrupted login was left for the next start"
            ),
            Outcome::Blocked(reason) => blocked.push(reason),
        }
    }

    if blocked.is_empty() {
        Recovery::Clean
    } else {
        unresolved(&blocked.join("; "))
    }
}

fn another_instance(lock: &str) -> Recovery {
    tracing::error!(
        "refusing to touch stores and credentials another running instance owns: {lock}"
    );
    Recovery::Blocked(UserMessage::about(
        UserMessageKind::AnotherInstanceRunning,
        &lock,
    ))
}

fn unclaimed(reason: &str) -> Recovery {
    tracing::error!("refusing to touch stores and credentials nothing guards: {reason}");
    Recovery::Blocked(UserMessage::about(
        UserMessageKind::LocalDataClaimFailed,
        &reason,
    ))
}

fn unlisted(error: &AppError) -> Recovery {
    let reason = format!("the interrupted logins could not be listed ({error})");
    tracing::error!("refusing to sign in past login journals it cannot see: {reason}");
    unresolved(&reason)
}

fn unresolved(reason: &str) -> Recovery {
    Recovery::Blocked(UserMessage::about(
        UserMessageKind::InterruptedLoginUnresolved,
        &reason,
    ))
}

async fn resolve(
    auth: &dyn AuthPort,
    storage: &dyn StoragePort,
    login: &InterruptedLogin,
) -> Outcome {
    let login = match login {
        InterruptedLogin::Journaled(login) => login,
        InterruptedLogin::Unreadable { txn, reason } => return unreadable(txn, reason),
    };
    match login.resolution {
        LoginResolution::RollBack => roll_back(auth, storage, login).await,
        LoginResolution::RollForward => roll_forward(auth, storage, login).await,
        LoginResolution::Close => closed(
            login,
            conclude::close_terminal(auth, storage, &login.txn).await,
        ),
    }
}

fn unreadable(txn: &str, reason: &str) -> Outcome {
    tracing::error!(
        txn,
        "refusing to sign in past a login journal that cannot be read: {reason}"
    );
    Outcome::Blocked(format!(
        "the journal of login {txn} cannot be read, so the store and credentials it protects \
         cannot be restored ({reason})"
    ))
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
    closed(
        login,
        conclude::close_terminal(auth, storage, &login.txn).await,
    )
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

    match conclude::close_rolled_back(auth, storage, &login.txn).await {
        Ok(closing) => closed(login, closing),
        Err(e) => {
            let reason = format!(
                "login {} was undone, but that could not be recorded, so its staged credentials \
                 are still needed ({e})",
                login.txn
            );
            tracing::error!(txn = %login.txn, "{reason}");
            Outcome::Blocked(reason)
        }
    }
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
    let staged = match plan {
        CredentialPlan::Restore(staged) => staged,
        CredentialPlan::NothingStaged => {
            tracing::info!(
                txn = %login.txn,
                "no credentials were staged for this login, leaving the credential store as it is"
            );
            return CleanupReport::default();
        }
    };

    credentials::restore_displaced(storage, &login.account, &staged.displaced).await
}

fn report_unresolved(login: &PendingLogin, report: &CleanupReport) {
    tracing::warn!(
        txn = %login.txn,
        "an interrupted login could not be resolved and will be retried at the next start: {}",
        report.summary()
    );
}

fn closed(login: &PendingLogin, closing: Closing) -> Outcome {
    match closing {
        Closing::Closed => {
            tracing::info!(txn = %login.txn, ?login.resolution, "interrupted login resolved");
            Outcome::Resolved
        }
        Closing::CleanupPending => Outcome::Retry,
    }
}
