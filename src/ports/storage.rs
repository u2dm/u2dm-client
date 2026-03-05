use async_trait::async_trait;

use crate::domain::models::Session;
use crate::error::Result;

#[async_trait]
pub trait StoragePort: Send + Sync {
    async fn save_session(&self, session: &Session) -> Result<()>;
    async fn load_session(&self) -> Result<Option<Session>>;
    async fn clear_session(&self) -> Result<()>;
    async fn save_passphrase(&self, passphrase: &str) -> Result<()>;
    async fn load_passphrase(&self) -> Result<Option<String>>;
}
