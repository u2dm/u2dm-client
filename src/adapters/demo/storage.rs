use async_trait::async_trait;

use super::{data, login};
use crate::domain::account::AccountScope;
use crate::domain::models::Session;
use crate::error::Result;
use crate::ports::storage::{StagedCredentials, StoragePort, StoredSession, SupersededLogin};

pub struct DemoStorage;

#[async_trait]
impl StoragePort for DemoStorage {
    async fn save_session(&self, _session: &Session) -> Result<()> {
        Ok(())
    }

    async fn load_session(&self) -> Result<StoredSession> {
        if login::requested().is_some_and(|demo| !demo.keeps_session) {
            return Ok(StoredSession::Absent);
        }
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

    async fn save_superseded(&self, _superseded: &SupersededLogin) -> Result<()> {
        Ok(())
    }

    async fn load_superseded(&self) -> Result<StagedCredentials> {
        Ok(StagedCredentials::Absent)
    }

    async fn clear_superseded(&self, _txn: &str) -> Result<()> {
        Ok(())
    }
}
