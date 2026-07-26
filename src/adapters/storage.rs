use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;
use tokio::task::spawn_blocking;

use crate::domain::account::AccountScope;
use crate::domain::models::Session;
use crate::error::{AppError, Result};
use crate::ports::storage::{StoragePort, StoredSession};

const KEYRING_SERVICE: &str = "u2dm";
const SESSION_KEY: &str = "session-credentials";
const SESSION_RECORD_VERSION: u8 = 2;

#[derive(serde::Deserialize)]
struct RecordVersion {
    version: u8,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredSessionRecord {
    version: u8,
    user_id: String,
    device_id: String,
    homeserver: String,
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_id: Option<String>,
}

impl StoredSessionRecord {
    fn new(session: &Session) -> Self {
        Self {
            version: SESSION_RECORD_VERSION,
            user_id: session.user_id.clone(),
            device_id: session.device_id.clone(),
            homeserver: session.homeserver.clone(),
            access_token: session.access_token.clone(),
            refresh_token: session.refresh_token.clone(),
            client_id: session.client_id.clone(),
        }
    }

    fn into_session(self) -> Session {
        Session {
            user_id: self.user_id,
            device_id: self.device_id,
            homeserver: self.homeserver,
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            client_id: self.client_id,
        }
    }
}

fn passphrase_key(account: &AccountScope) -> String {
    format!("db-passphrase-{}", account.id())
}

fn combine(operation: &str, failures: &[String]) -> Result<()> {
    if failures.is_empty() {
        Ok(())
    } else {
        Err(AppError::Other(format!(
            "{operation}: {}",
            failures.join("; ")
        )))
    }
}

pub struct SecureStorage {
    superseded_metadata_path: PathBuf,
}

impl SecureStorage {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            superseded_metadata_path: data_dir.join("session.json"),
        }
    }

    async fn drop_superseded_metadata(&self) -> io::Result<()> {
        match fs::remove_file(&self.superseded_metadata_path).await {
            Ok(()) => {
                tracing::debug!("removed session metadata left by an earlier layout");
                Ok(())
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[async_trait]
impl StoragePort for SecureStorage {
    async fn save_session(&self, session: &Session) -> Result<()> {
        tracing::debug!(user_id = %session.user_id, "saving session");
        write_record(&StoredSessionRecord::new(session)).await?;

        if let Err(e) = self.drop_superseded_metadata().await {
            tracing::warn!("stale session metadata could not be removed: {e}");
        }
        tracing::debug!("session saved");

        Ok(())
    }

    async fn load_session(&self) -> Result<StoredSession> {
        let raw = match keyring_get(SESSION_KEY).await {
            Ok(Some(raw)) => raw,
            Ok(None) => return Ok(StoredSession::Absent),
            Err(e) => {
                tracing::warn!("keyring unavailable while loading session: {e}");
                return Ok(StoredSession::CredentialsUnavailable(e));
            }
        };

        match decode_record(&raw) {
            Some(record) => Ok(StoredSession::Present(record.into_session())),
            None => Ok(StoredSession::Incomplete),
        }
    }

    async fn clear_session(&self) -> Result<()> {
        tracing::debug!("clearing stored session");
        let mut failures = Vec::new();

        if let Err(e) = keyring_delete(SESSION_KEY).await {
            failures.push(format!("{SESSION_KEY} ({e})"));
        }

        if let Err(e) = self.drop_superseded_metadata().await {
            failures.push(format!("{} ({e})", self.superseded_metadata_path.display()));
        }

        combine("stored credentials could not be removed", &failures)
    }

    async fn save_passphrase(&self, account: &AccountScope, passphrase: &str) -> Result<()> {
        keyring_set(&passphrase_key(account), passphrase.to_owned()).await
    }

    async fn load_passphrase(&self, account: &AccountScope) -> Result<Option<String>> {
        keyring_get(&passphrase_key(account)).await
    }

    async fn clear_passphrase(&self, account: &AccountScope) -> Result<()> {
        keyring_delete(&passphrase_key(account)).await
    }
}

async fn write_record(record: &StoredSessionRecord) -> Result<()> {
    keyring_set(SESSION_KEY, serde_json::to_string(record)?).await
}

fn decode_record(raw: &str) -> Option<StoredSessionRecord> {
    match serde_json::from_str::<RecordVersion>(raw) {
        Ok(RecordVersion { version }) if version == SESSION_RECORD_VERSION => {}
        Ok(RecordVersion { version }) => {
            tracing::warn!(
                version,
                "the stored session uses an unsupported layout, re-login required"
            );
            return None;
        }
        Err(e) => {
            tracing::warn!("the stored session is unreadable, re-login required: {e}");
            return None;
        }
    }

    match serde_json::from_str(raw) {
        Ok(record) => Some(record),
        Err(e) => {
            tracing::warn!("the stored session is incomplete, re-login required: {e}");
            None
        }
    }
}

async fn keyring_set(key: &str, secret: String) -> Result<()> {
    let key = key.to_owned();
    spawn_blocking(move || {
        let entry = open_entry(&key)?;
        entry
            .set_password(&secret)
            .map_err(|source| AppError::Keyring { key, source })
    })
    .await
    .map_err(|e| AppError::Other(e.to_string()))?
}

async fn keyring_get(key: &str) -> Result<Option<String>> {
    let key = key.to_owned();
    spawn_blocking(move || {
        let entry = open_entry(&key)?;
        match entry.get_password() {
            Ok(pw) => Ok(Some(pw)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(source) => Err(AppError::Keyring { key, source }),
        }
    })
    .await
    .map_err(|e| AppError::Other(e.to_string()))?
}

async fn keyring_delete(key: &str) -> Result<()> {
    let key = key.to_owned();
    spawn_blocking(move || {
        let entry = open_entry(&key)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(source) => Err(AppError::Keyring { key, source }),
        }
    })
    .await
    .map_err(|e| AppError::Other(e.to_string()))?
}

fn open_entry(key: &str) -> Result<keyring_core::Entry> {
    ensure_default_store();
    keyring_core::Entry::new(KEYRING_SERVICE, key).map_err(|source| AppError::Keyring {
        key: key.to_owned(),
        source,
    })
}

fn ensure_default_store() {
    if keyring_core::get_default_store().is_some() {
        return;
    }
    if let Err(e) = register_default_store() {
        tracing::warn!("failed to initialize keyring credential store: {e}");
    }
}

fn register_default_store() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let store = apple_native_keyring_store::keychain::Store::new().map_err(store_error)?;
        keyring_core::set_default_store(store);
    }
    #[cfg(target_os = "windows")]
    {
        let store = windows_native_keyring_store::Store::new().map_err(store_error)?;
        keyring_core::set_default_store(store);
    }
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    {
        let store = zbus_secret_service_keyring_store::Store::new().map_err(store_error)?;
        keyring_core::set_default_store(store);
    }
    Ok(())
}

#[cfg(any(
    target_os = "macos",
    target_os = "windows",
    all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    )
))]
fn store_error(source: keyring_core::Error) -> AppError {
    AppError::Keyring {
        key: "<default-store>".to_owned(),
        source,
    }
}
