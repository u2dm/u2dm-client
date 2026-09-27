use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use super::data;
use super::login::{self, SavedSession};
use crate::domain::account::AccountScope;
use crate::domain::auth::Session;
use crate::error::{AppError, Result};
use crate::ports::storage::{StagedCredentials, StoragePort, StoredSession, SupersededLogin};

const SESSION_ENTRY: &str = "session-credentials";

pub struct DemoStorage {
    session_locked: AtomicBool,
    store_key_locked: AtomicBool,
}

impl DemoStorage {
    pub fn from_env() -> Self {
        let locked = saved_session() == SavedSession::BehindLockedKeyring;
        Self {
            session_locked: AtomicBool::new(locked),
            store_key_locked: AtomicBool::new(locked),
        }
    }
}

fn saved_session() -> SavedSession {
    login::requested().map_or(SavedSession::Restorable, |demo| demo.saved_session)
}

fn locked_keyring(key: String) -> AppError {
    AppError::Keyring {
        key,
        source: keyring_core::Error::NoStorageAccess("the demo keyring is locked".into()),
    }
}

#[async_trait]
impl StoragePort for DemoStorage {
    async fn save_session(&self, _session: &Session) -> Result<()> {
        Ok(())
    }

    async fn load_session(&self) -> Result<StoredSession> {
        if saved_session() == SavedSession::Absent {
            return Ok(StoredSession::Absent);
        }
        if self.session_locked.swap(false, Ordering::Relaxed) {
            return Ok(StoredSession::CredentialsUnavailable(locked_keyring(
                SESSION_ENTRY.to_owned(),
            )));
        }
        Ok(StoredSession::Present(data::session()))
    }

    async fn clear_session(&self) -> Result<()> {
        Ok(())
    }

    async fn save_passphrase(&self, _account: &AccountScope, _passphrase: &str) -> Result<()> {
        Ok(())
    }

    async fn load_passphrase(&self, account: &AccountScope) -> Result<Option<String>> {
        if self.store_key_locked.swap(false, Ordering::Relaxed) {
            return Err(locked_keyring(format!("db-passphrase-{}", account.id())));
        }
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
