use async_trait::async_trait;

use crate::domain::models::Session;
use crate::error::{AppError, Result};

pub enum StoredSession {
    Absent,
    Present(Session),
    Incomplete,
    CredentialsUnavailable(AppError),
}

#[async_trait]
pub trait StoragePort: Send + Sync {
    async fn save_session(&self, session: &Session) -> Result<()>;
    async fn load_session(&self) -> Result<StoredSession>;
    async fn clear_session(&self) -> Result<()>;
    async fn save_passphrase(&self, passphrase: &str) -> Result<()>;
    async fn load_passphrase(&self) -> Result<Option<String>>;
    async fn save_space_order(&self, order: &[String]) -> Result<()>;
    async fn load_space_order(&self) -> Result<Vec<String>>;
}
