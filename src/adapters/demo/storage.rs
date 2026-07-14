use std::sync::Mutex;

use async_trait::async_trait;

use super::data;
use crate::domain::models::Session;
use crate::error::Result;
use crate::ports::storage::StoragePort;

#[derive(Default)]
pub struct DemoStorage {
    space_order: Mutex<Vec<String>>,
}

#[async_trait]
impl StoragePort for DemoStorage {
    async fn save_session(&self, _session: &Session) -> Result<()> {
        Ok(())
    }

    async fn load_session(&self) -> Result<Option<Session>> {
        Ok(Some(data::session()))
    }

    async fn clear_session(&self) -> Result<()> {
        Ok(())
    }

    async fn save_passphrase(&self, _passphrase: &str) -> Result<()> {
        Ok(())
    }

    async fn load_passphrase(&self) -> Result<Option<String>> {
        Ok(Some("demo-passphrase".to_owned()))
    }

    async fn save_space_order(&self, order: &[String]) -> Result<()> {
        if let Ok(mut stored) = self.space_order.lock() {
            *stored = order.to_vec();
        }
        Ok(())
    }

    async fn load_space_order(&self) -> Result<Vec<String>> {
        Ok(self
            .space_order
            .lock()
            .map(|order| order.clone())
            .unwrap_or_default())
    }
}
