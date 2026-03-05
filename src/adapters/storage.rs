use std::io::ErrorKind;
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
        let metadata = session.metadata();
        write_metadata(&self.session_path, &metadata).await?;

        if let Err(e) = keyring_set("access-token", session.access_token.clone()).await {
            tracing::warn!("failed to store access token in keyring: {e}");
        }

        match &session.refresh_token {
            Some(token) => {
                if let Err(e) = keyring_set("refresh-token", token.clone()).await {
                    tracing::warn!("failed to store refresh token in keyring: {e}");
                }
            }
            None => {
                if let Err(e) = keyring_delete("refresh-token").await {
                    tracing::warn!("failed to clear refresh token from keyring: {e}");
                }
            }
        }

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
        let entry = keyring::Entry::new(KEYRING_SERVICE, &key).map_err(|e| AppError::Keyring {
            key: key.clone(),
            source: e,
        })?;
        entry
            .set_password(&secret)
            .map_err(|e| AppError::Keyring { key, source: e })
    })
    .await
    .map_err(|e| AppError::Other(e.to_string()))?
}

async fn keyring_get(key: &str) -> Result<Option<String>> {
    let key = key.to_owned();
    spawn_blocking(move || {
        let entry = keyring::Entry::new(KEYRING_SERVICE, &key).map_err(|e| AppError::Keyring {
            key: key.clone(),
            source: e,
        })?;
        match entry.get_password() {
            Ok(pw) => Ok(Some(pw)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(AppError::Keyring { key, source: e }),
        }
    })
    .await
    .map_err(|e| AppError::Other(e.to_string()))?
}

async fn keyring_delete(key: &str) -> Result<()> {
    let key = key.to_owned();
    spawn_blocking(move || {
        let entry = keyring::Entry::new(KEYRING_SERVICE, &key).map_err(|e| AppError::Keyring {
            key: key.clone(),
            source: e,
        })?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(AppError::Keyring { key, source: e }),
        }
    })
    .await
    .map_err(|e| AppError::Other(e.to_string()))?
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
        use std::fs::Permissions;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp_path, Permissions::from_mode(0o600)).await?;
    }

    fs::rename(&tmp_path, path).await?;

    Ok(())
}
