use crate::error::Result;
use crate::ports::matrix::AuthPort;
use crate::ports::storage::StoragePort;

#[derive(Clone, Copy)]
pub(super) enum Closing {
    Closed,
    CleanupPending,
}

pub(super) async fn close_rolled_back(
    auth: &dyn AuthPort,
    storage: &dyn StoragePort,
    txn: &str,
) -> Result<Closing> {
    auth.mark_rolled_back(txn).await?;
    Ok(close_terminal(auth, storage, txn).await)
}

pub(super) async fn close_terminal(
    auth: &dyn AuthPort,
    storage: &dyn StoragePort,
    txn: &str,
) -> Closing {
    if let Err(e) = storage.clear_superseded(txn).await {
        tracing::warn!(
            txn,
            "the staged credentials could not be unstaged, so the login stays open: {e}"
        );
        return Closing::CleanupPending;
    }
    auth.forget_login(txn).await;
    Closing::Closed
}
