use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use async_trait::async_trait;
use tokio::fs;
use tokio::runtime::Handle;
use tokio::sync::oneshot;

use crate::domain::account::AccountScope;
use crate::domain::auth::Session;
use crate::error::{AppError, Result};
use crate::ports::storage::{
    DisplacedCredentials, StagedCredentials, StoragePort, StoredSession, SupersededLogin,
};

const KEYRING_SERVICE: &str = "u2dm";
const SESSION_KEY: &str = "session-credentials";
const SUPERSEDED_KEY: &str = "superseded-login";
const SESSION_RECORD_VERSION: u8 = 2;
const SUPERSEDED_RECORD_VERSION: u8 = 1;

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

#[derive(serde::Serialize, serde::Deserialize)]
struct SupersededRecord {
    version: u8,
    txn: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<StoredSessionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    passphrase: Option<String>,
}

impl SupersededRecord {
    fn new(superseded: &SupersededLogin) -> Self {
        Self {
            version: SUPERSEDED_RECORD_VERSION,
            txn: superseded.txn.clone(),
            session: superseded
                .displaced
                .session
                .as_ref()
                .map(StoredSessionRecord::new),
            passphrase: superseded.displaced.passphrase.clone(),
        }
    }

    fn into_superseded(self) -> SupersededLogin {
        SupersededLogin {
            txn: self.txn,
            displaced: DisplacedCredentials {
                session: self.session.map(StoredSessionRecord::into_session),
                passphrase: self.passphrase,
            },
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
    keyring: KeyringLane,
    superseded_metadata_path: PathBuf,
}

impl SecureStorage {
    pub fn new(data_dir: &Path, runtime: &Handle) -> Self {
        Self {
            keyring: KeyringLane::new(runtime),
            superseded_metadata_path: data_dir.join("session.json"),
        }
    }

    async fn write_session_record(&self, record: &StoredSessionRecord) -> Result<()> {
        self.keyring
            .set(SESSION_KEY, serde_json::to_string(record)?)
            .await
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
        self.write_session_record(&StoredSessionRecord::new(session))
            .await?;

        if let Err(e) = self.drop_superseded_metadata().await {
            tracing::warn!("stale session metadata could not be removed: {e}");
        }
        tracing::debug!("session saved");

        Ok(())
    }

    async fn load_session(&self) -> Result<StoredSession> {
        let raw = match self.keyring.get(SESSION_KEY).await {
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

        if let Err(e) = self.keyring.delete(SESSION_KEY).await {
            failures.push(format!("{SESSION_KEY} ({e})"));
        }

        if let Err(e) = self.drop_superseded_metadata().await {
            failures.push(format!("{} ({e})", self.superseded_metadata_path.display()));
        }

        combine("stored credentials could not be removed", &failures)
    }

    async fn save_passphrase(&self, account: &AccountScope, passphrase: &str) -> Result<()> {
        self.keyring
            .set(&passphrase_key(account), passphrase.to_owned())
            .await
    }

    async fn load_passphrase(&self, account: &AccountScope) -> Result<Option<String>> {
        self.keyring.get(&passphrase_key(account)).await
    }

    async fn clear_passphrase(&self, account: &AccountScope) -> Result<()> {
        self.keyring.delete(&passphrase_key(account)).await
    }

    async fn save_superseded(&self, superseded: &SupersededLogin) -> Result<()> {
        tracing::debug!(txn = %superseded.txn, "staging the credentials this login displaces");
        let encoded = serde_json::to_string(&SupersededRecord::new(superseded))?;
        self.keyring.set(SUPERSEDED_KEY, encoded).await
    }

    async fn load_superseded(&self) -> Result<StagedCredentials> {
        let Some(raw) = self.keyring.get(SUPERSEDED_KEY).await? else {
            return Ok(StagedCredentials::Absent);
        };
        Ok(decode_superseded(&raw))
    }

    async fn clear_superseded(&self, txn: &str) -> Result<()> {
        let Some(raw) = self.keyring.get(SUPERSEDED_KEY).await? else {
            return Ok(());
        };
        match decode_superseded(&raw) {
            StagedCredentials::Present(staged) if staged.txn != txn => {
                tracing::warn!(
                    txn,
                    staged = %staged.txn,
                    "the staged credentials belong to another login, leaving them in place"
                );
                Ok(())
            }
            _ => self.keyring.delete(SUPERSEDED_KEY).await,
        }
    }
}

fn decode_superseded(raw: &str) -> StagedCredentials {
    match serde_json::from_str::<SupersededRecord>(raw) {
        Ok(record) if record.version == SUPERSEDED_RECORD_VERSION => {
            StagedCredentials::Present(record.into_superseded())
        }
        Ok(record) => {
            tracing::warn!(
                version = record.version,
                "the staged displaced credentials use an unsupported layout"
            );
            StagedCredentials::Corrupt
        }
        Err(e) => {
            tracing::warn!("the staged displaced credentials are unreadable: {e}");
            StagedCredentials::Corrupt
        }
    }
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

type KeyringCall = Box<dyn FnOnce() + Send>;

struct KeyringLane {
    queue: mpsc::Sender<KeyringCall>,
}

impl KeyringLane {
    fn new(runtime: &Handle) -> Self {
        let (queue, calls) = mpsc::channel::<KeyringCall>();
        runtime.spawn_blocking(move || {
            while let Ok(call) = calls.recv() {
                call();
            }
            tracing::debug!("the keyring lane drained and closed");
        });
        Self { queue }
    }

    async fn in_order<T: Send + 'static>(
        &self,
        key: &str,
        call: impl FnOnce(String) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let owned = key.to_owned();
        let (finished, settled) = oneshot::channel();
        self.queue
            .send(Box::new(move || drop(finished.send(call(owned)))))
            .map_err(|_| lane_stopped(key))?;
        settled.await.map_err(|_| lane_stopped(key))?
    }

    async fn set(&self, key: &str, secret: String) -> Result<()> {
        self.in_order(key, move |key| {
            let entry = open_entry(&key)?;
            entry
                .set_password(&secret)
                .map_err(|source| AppError::Keyring { key, source })
        })
        .await
    }

    async fn get(&self, key: &str) -> Result<Option<String>> {
        self.in_order(key, move |key| {
            let entry = open_entry(&key)?;
            match entry.get_password() {
                Ok(pw) => Ok(Some(pw)),
                Err(keyring_core::Error::NoEntry) => Ok(None),
                Err(source) => Err(AppError::Keyring { key, source }),
            }
        })
        .await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.in_order(key, move |key| {
            let entry = open_entry(&key)?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
                Err(source) => Err(AppError::Keyring { key, source }),
            }
        })
        .await
    }
}

fn lane_stopped(key: &str) -> AppError {
    AppError::Other(format!("the keyring lane stopped before it reached {key}"))
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
