use async_trait::async_trait;

use crate::domain::account::AccountScope;
use crate::domain::models::Session;
use crate::error::{AppError, Result};

pub enum StoredSession {
    Absent,
    Present(Session),
    Incomplete,
    CredentialsUnavailable(AppError),
}

pub struct SupersededLogin {
    pub txn: String,
    pub session: Option<Session>,
    pub passphrase: Option<String>,
}

pub enum StagedCredentials {
    Absent,
    Present(SupersededLogin),
    Corrupt,
}

#[async_trait]
pub trait StoragePort: Send + Sync {
    async fn save_session(&self, session: &Session) -> Result<()>;
    async fn load_session(&self) -> Result<StoredSession>;
    async fn clear_session(&self) -> Result<()>;
    async fn save_passphrase(&self, account: &AccountScope, passphrase: &str) -> Result<()>;
    async fn load_passphrase(&self, account: &AccountScope) -> Result<Option<String>>;
    async fn clear_passphrase(&self, account: &AccountScope) -> Result<()>;
    async fn save_superseded(&self, superseded: &SupersededLogin) -> Result<()>;
    async fn load_superseded(&self) -> Result<StagedCredentials>;
    async fn clear_superseded(&self, txn: &str) -> Result<()>;
}
