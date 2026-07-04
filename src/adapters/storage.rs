#[cfg(unix)]
use std::fs::Permissions;
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;
use tokio::task::spawn_blocking;

use crate::domain::models::{Session, SessionMetadata};
use crate::error::{AppError, Result};
use crate::ports::storage::StoragePort;

const KEYRING_SERVICE: &str = "u2dm";

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
        keyring_set("access-token", session.access_token.clone()).await?;

        match &session.refresh_token {
            Some(token) => keyring_set("refresh-token", token.clone()).await?,
            None => keyring_delete("refresh-token").await?,
        }

        let metadata = session.metadata();
        write_metadata(&self.session_path, &metadata).await?;
        tracing::debug!("session saved");

        Ok(())
    }

    async fn load_session(&self) -> Result<Option<Session>> {
        let contents = match fs::read_to_string(&self.session_path).await {
            Ok(c) => c,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        let metadata: SessionMetadata = serde_json::from_str(&contents)?;

        let access_token = match keyring_get("access-token").await {
            Ok(Some(token)) => token,
            Ok(None) => {
                tracing::info!("no access token in keyring, re-login required");
                return Ok(None);
            }
            Err(e) => {
                tracing::warn!("keyring unavailable, re-login required: {e}");
                return Ok(None);
            }
        };

        let refresh_token = match keyring_get("refresh-token").await {
            Ok(token) => token,
            Err(e) => {
                tracing::warn!("failed to load refresh token from keyring: {e}");
                None
            }
        };

        Ok(Some(Session {
            user_id: metadata.user_id,
            device_id: metadata.device_id,
            homeserver: metadata.homeserver,
            access_token,
            refresh_token,
            client_id: metadata.client_id,
        }))
    }

    async fn clear_session(&self) -> Result<()> {
        tracing::debug!("clearing stored session");
        match fs::remove_file(&self.session_path).await {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }

        for key in ["access-token", "refresh-token"] {
            if let Err(e) = keyring_delete(key).await {
                tracing::warn!("failed to clear {key} from keyring: {e}");
            }
        }

        Ok(())
    }

    async fn save_passphrase(&self, passphrase: &str) -> Result<()> {
        keyring_set("db-passphrase", passphrase.to_owned()).await
    }

    async fn load_passphrase(&self) -> Result<Option<String>> {
        keyring_get("db-passphrase").await
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

async fn write_metadata(path: &Path, metadata: &SessionMetadata) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let json = serde_json::to_string_pretty(metadata)?;
    let tmp_path = path.with_extension("tmp");

    fs::write(&tmp_path, json.as_bytes()).await?;

    #[cfg(unix)]
    {
        fs::set_permissions(&tmp_path, Permissions::from_mode(0o600)).await?;
    }

    fs::rename(&tmp_path, path).await?;

    Ok(())
}
