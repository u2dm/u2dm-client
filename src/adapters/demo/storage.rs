use async_trait::async_trait;

use super::data;
use crate::domain::account::AccountScope;
use crate::domain::models::Session;
use crate::error::Result;
use crate::ports::storage::{StoragePort, StoredSession};

pub struct DemoStorage;

#[async_trait]
impl StoragePort for DemoStorage {
    async fn save_session(&self, _session: &Session) -> Result<()> {
        Ok(())
    }

    async fn load_session(&self) -> Result<StoredSession> {
        Ok(StoredSession::Present(data::session()))
    }

    async fn clear_session(&self) -> Result<()> {
        Ok(())
    }

    async fn save_passphrase(&self, _account: &AccountScope, _passphrase: &str) -> Result<()> {
        Ok(())
    }

    async fn load_passphrase(&self, _account: &AccountScope) -> Result<Option<String>> {
        Ok(Some("demo-passphrase".to_owned()))
    }

    async fn clear_passphrase(&self, _account: &AccountScope) -> Result<()> {
        Ok(())
    }
}
