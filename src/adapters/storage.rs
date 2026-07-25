#[cfg(unix)]
use std::fs::Permissions;
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;
use tokio::task::spawn_blocking;

use crate::domain::account::AccountScope;
use crate::domain::models::{Session, SessionMetadata};
use crate::error::{AppError, Result};
use crate::ports::storage::{StoragePort, StoredSession};
use crate::util::unique_tmp_path;

const KEYRING_SERVICE: &str = "u2dm";
const CREDENTIALS_KEY: &str = "session-credentials";
const CREDENTIALS_VERSION: u8 = 1;

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredCredentials {
    version: u8,
    user_id: String,
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
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
    session_path: PathBuf,
}

impl SecureStorage {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            session_path: data_dir.join("session.json"),
        }
    }
}

#[async_trait]
impl StoragePort for SecureStorage {
    async fn save_session(&self, session: &Session) -> Result<()> {
        tracing::debug!(user_id = %session.user_id, "saving session");
        write_credentials(&StoredCredentials {
            version: CREDENTIALS_VERSION,
            user_id: session.user_id.clone(),
            access_token: session.access_token.clone(),
            refresh_token: session.refresh_token.clone(),
        })
        .await?;

        let metadata = session.metadata();
        write_json(&self.session_path, &metadata).await?;
        tracing::debug!("session saved");

        Ok(())
    }

    async fn load_session(&self) -> Result<StoredSession> {
        let contents = match fs::read_to_string(&self.session_path).await {
            Ok(c) => c,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(StoredSession::Absent),
            Err(e) => return Err(e.into()),
        };

        let metadata: SessionMetadata = serde_json::from_str(&contents)?;

        let credentials = match load_credentials(&metadata.user_id).await {
            Ok(Some(credentials)) => credentials,
            Ok(None) => {
                tracing::info!("session metadata present but no usable credentials in keyring");
                return Ok(StoredSession::Incomplete);
            }
            Err(e) => {
                tracing::warn!("keyring unavailable while loading session: {e}");
                return Ok(StoredSession::CredentialsUnavailable(e));
            }
        };

        Ok(StoredSession::Present(Session {
            user_id: metadata.user_id,
            device_id: metadata.device_id,
            homeserver: metadata.homeserver,
            access_token: credentials.access_token,
            refresh_token: credentials.refresh_token,
            client_id: metadata.client_id,
        }))
    }

    async fn clear_session(&self) -> Result<()> {
        tracing::debug!("clearing stored session");
        let mut failures = Vec::new();

        match fs::remove_file(&self.session_path).await {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => failures.push(format!("{} ({e})", self.session_path.display())),
        }

        if let Err(e) = keyring_delete(CREDENTIALS_KEY).await {
            failures.push(format!("{CREDENTIALS_KEY} ({e})"));
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

async fn write_credentials(record: &StoredCredentials) -> Result<()> {
    keyring_set(CREDENTIALS_KEY, serde_json::to_string(record)?).await
}

async fn load_credentials(user_id: &str) -> Result<Option<StoredCredentials>> {
    Ok(keyring_get(CREDENTIALS_KEY)
        .await?
        .and_then(|raw| decode_credentials(&raw, user_id)))
}

fn decode_credentials(raw: &str, user_id: &str) -> Option<StoredCredentials> {
    let record: StoredCredentials = match serde_json::from_str(raw) {
        Ok(record) => record,
        Err(e) => {
            tracing::warn!("stored credentials are unreadable, re-login required: {e}");
            return None;
        }
    };

    if record.version != CREDENTIALS_VERSION {
        tracing::warn!(
            version = record.version,
            "stored credentials use an unsupported layout, re-login required"
        );
        return None;
    }

    if record.user_id != user_id {
        tracing::warn!("stored credentials belong to a different account, re-login required");
        return None;
    }

    Some(record)
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

async fn write_json<T: serde::Serialize + Sync + ?Sized>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let json = serde_json::to_string_pretty(value)?;
    let tmp_path = unique_tmp_path(path);

    fs::write(&tmp_path, json.as_bytes()).await?;

    #[cfg(unix)]
    {
        fs::set_permissions(&tmp_path, Permissions::from_mode(0o600)).await?;
    }

    if let Err(e) = fs::rename(&tmp_path, path).await {
        if let Err(cleanup_err) = fs::remove_file(&tmp_path).await {
            tracing::debug!("failed to remove staged temp file: {cleanup_err}");
        }
        return Err(e.into());
    }

    Ok(())
}
