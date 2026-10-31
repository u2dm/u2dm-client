use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use futures_util::stream::{SelectAll, select_all};
use futures_util::{Stream, StreamExt};
use matrix_sdk::Client;
use matrix_sdk::encryption::identities::UserIdentity;
use matrix_sdk::encryption::recovery::{RecoveryError, RecoveryState};
use matrix_sdk::encryption::secret_storage::SecretStorageError;
use matrix_sdk::encryption::{
    CrossSigningResetAuthType, KeyExportError, OAuthCrossSigningResetInfo, RoomKeyImportError,
};
use matrix_sdk::ruma::api::client::keys::get_keys;
use matrix_sdk::ruma::events::GlobalAccountDataEventType;
use matrix_sdk::ruma::events::secret::request::SecretName;
use matrix_sdk::ruma::{OwnedUserId, UserId};
use matrix_sdk_base::crypto::secret_storage::DecodeError;
use tokio::sync::{Notify, mpsc};
use tokio::task::spawn_blocking;

use super::session::ClientHandle;
use super::uia;
use super::user_info::own_user;
use crate::adapters::private_fs;
use crate::domain::encryption::{
    AccountEncryption, IdentityOutcome, KeyImport, KeyPassphrase, KeyStorage, Recovery,
    RecoveryAttempt, RecoveryKey, RecoverySetup, SessionTrust,
};
use crate::domain::uia::{AuthAnswer, AuthChallenge, Guarded, OAUTH_STAGE};
use crate::error::{AppError, Result};
use crate::ports::matrix::EncryptionPort;

const KEY_BACKUP_PREFERENCE: &str = "m.key_backup";
const UNSTABLE_BACKUP_DISABLED: &str = "m.org.matrix.custom.backup_disabled";

pub(super) struct MatrixEncryption {
    matrix: Arc<ClientHandle>,
    changed: Notify,
    unpublished: Unpublished,
    resetting: AtomicBool,
}

impl MatrixEncryption {
    pub(super) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self {
            matrix,
            changed: Notify::new(),
            unpublished: Unpublished::default(),
            resetting: AtomicBool::new(false),
        }
    }

    async fn keep_published_identity(&self, client: &Client) {
        let Ok(own) = own_user(client) else {
            return;
        };
        self.unpublished.remember(client, &own).await;
        read_own_identity_back(client, &own).await;
        self.changed.notify_one();
    }
}

#[derive(Default)]
struct Unpublished {
    master_key: StdMutex<Option<String>>,
}

impl Unpublished {
    async fn remember(&self, client: &Client, own: &UserId) {
        let local = client.encryption().get_user_identity(own).await;
        let master = local.ok().flatten().map(|identity| master_key(&identity));
        if let Ok(mut unpublished) = self.master_key.lock() {
            unpublished.clone_from(&master);
        }
        tracing::debug!(
            ?master,
            "the identity this session holds is not on the homeserver"
        );
    }

    fn forget(&self) {
        if let Ok(mut unpublished) = self.master_key.lock() {
            *unpublished = None;
        }
    }

    fn holds(&self, identity: &UserIdentity) -> bool {
        self.master_key
            .lock()
            .is_ok_and(|unpublished| unpublished.as_deref() == Some(master_key(identity).as_str()))
    }
}

pub(super) fn master_key(identity: &UserIdentity) -> String {
    identity
        .master_key()
        .get_first_key()
        .map(|key| key.to_base64())
        .unwrap_or_default()
}

#[async_trait]
impl EncryptionPort for MatrixEncryption {
    async fn follow(&self, state_tx: mpsc::Sender<AccountEncryption>) -> Result<()> {
        let client = self.matrix.client().await?;
        follow(&client, &self.changed, &self.unpublished, state_tx).await
    }

    async fn set_up_identity(&self, auth: Option<&AuthAnswer>) -> Result<IdentityOutcome> {
        let outcome = set_up_identity(&self.matrix.client().await?, &self.unpublished, auth).await;
        self.changed.notify_one();
        outcome
    }

    async fn recover(&self, key: &RecoveryKey) -> Result<RecoveryAttempt> {
        let client = self.matrix.client().await?;
        let attempt = match client.encryption().recovery().recover(key.expose()).await {
            Ok(()) => Ok(RecoveryAttempt::Unlocked),
            Err(error) => failed_recovery(&error),
        };
        self.changed.notify_one();
        if let Ok(attempt) = &attempt {
            tracing::info!(?attempt, "tried the recovery key");
        }
        attempt
    }

    async fn enable_recovery(&self) -> Result<RecoverySetup> {
        let client = self.matrix.client().await?;
        let setup = match client.encryption().recovery().enable().await {
            Ok(key) => {
                tracing::info!("set up recovery and the key backup");
                Ok(RecoverySetup::Created(RecoveryKey::new(key)))
            }
            Err(RecoveryError::BackupExistsOnServer) => {
                tracing::warn!("a key backup this session cannot open is in the way of recovery");
                Ok(RecoverySetup::BackupInTheWay)
            }
            Err(error) => Err(AppError::Other(format!(
                "could not set up recovery: {error}"
            ))),
        };
        self.changed.notify_one();
        setup
    }

    async fn change_recovery_key(&self) -> Result<RecoveryKey> {
        let client = self.matrix.client().await?;
        let key = client
            .encryption()
            .recovery()
            .reset_key()
            .await
            .map_err(|e| AppError::Other(format!("could not change the recovery key: {e}")))?;
        tracing::info!("changed the recovery key");
        self.changed.notify_one();
        Ok(RecoveryKey::new(key))
    }

    async fn reset_identity(&self) -> Result<Guarded> {
        let client = self.matrix.client().await?;
        self.resetting.store(true, Ordering::SeqCst);
        let started = client.encryption().recovery().reset_identity().await;
        self.changed.notify_one();
        match started {
            Ok(None) => {
                self.resetting.store(false, Ordering::SeqCst);
                identity_replaced(&client, &self.unpublished).await;
                Ok(Guarded::Done)
            }
            Ok(Some(handle)) => {
                tracing::info!(
                    "the homeserver wants the user to authenticate before it takes the new identity"
                );
                Ok(Guarded::NeedsAuth(Box::new(reset_challenge(
                    &client,
                    handle.auth_type(),
                ))))
            }
            Err(e) => {
                self.resetting.store(false, Ordering::SeqCst);
                self.keep_published_identity(&client).await;
                Err(AppError::Other(format!(
                    "could not reset the identity: {e}"
                )))
            }
        }
    }

    async fn finish_identity_reset(&self, auth: &AuthAnswer) -> Result<Guarded> {
        let client = self.matrix.client().await?;
        let auth_data = uia::auth_data(&client, auth)?;
        let uploaded = client.encryption().bootstrap_cross_signing(auth_data).await;
        self.changed.notify_one();
        match uploaded {
            Ok(()) => {
                self.resetting.store(false, Ordering::SeqCst);
                identity_replaced(&client, &self.unpublished).await;
                Ok(Guarded::Done)
            }
            Err(error) => {
                if let Some(info) = error.as_uiaa_response() {
                    return Ok(Guarded::NeedsAuth(Box::new(uia::challenge(&client, info))));
                }
                self.resetting.store(false, Ordering::SeqCst);
                self.keep_published_identity(&client).await;
                Err(AppError::Other(format!(
                    "could not publish the new identity: {error}"
                )))
            }
        }
    }

    async fn abandon_identity_reset(&self) {
        if !self.resetting.swap(false, Ordering::SeqCst) {
            return;
        }
        tracing::info!("abandoning the identity reset, keeping the identity the homeserver holds");
        if let Ok(client) = self.matrix.client().await {
            self.keep_published_identity(&client).await;
        }
    }

    async fn export_room_keys(&self, path: &Path, passphrase: &KeyPassphrase) -> Result<usize> {
        let client = self.matrix.client().await?;
        let destination = path.to_path_buf();
        spawn_blocking(move || private_fs::create_or_open_private_blocking(&destination))
            .await
            .map_err(|e| AppError::Other(format!("could not create the key file: {e}")))??;
        let mut exported = 0;
        client
            .encryption()
            .export_room_keys(path.to_path_buf(), passphrase.expose(), |_| {
                exported += 1;
                true
            })
            .await
            .map_err(|e| AppError::Other(format!("could not export the room keys: {e}")))?;
        Ok(exported)
    }

    async fn import_room_keys(&self, path: &Path, passphrase: &KeyPassphrase) -> Result<KeyImport> {
        let client = self.matrix.client().await?;
        let imported = client
            .encryption()
            .import_room_keys(path.to_path_buf(), passphrase.expose())
            .await;
        self.changed.notify_one();
        match imported {
            Ok(result) => Ok(KeyImport::Imported {
                imported: result.imported_count,
                total: result.total_count,
            }),
            Err(RoomKeyImportError::Export(KeyExportError::InvalidMac)) => {
                Ok(KeyImport::WrongPassphrase)
            }
            Err(RoomKeyImportError::Export(
                KeyExportError::InvalidHeaders
                | KeyExportError::UnsupportedVersion
                | KeyExportError::InvalidUtf8(_)
                | KeyExportError::Json(_)
                | KeyExportError::Decode(_),
            )) => Ok(KeyImport::NotAKeyFile),
            Err(e) => Err(AppError::Other(format!(
                "could not import the room keys: {e}"
            ))),
        }
    }
}

fn reset_challenge(client: &Client, auth: &CrossSigningResetAuthType) -> AuthChallenge {
    match auth {
        CrossSigningResetAuthType::Uiaa(info) => uia::challenge(client, info),
        CrossSigningResetAuthType::OAuth(OAuthCrossSigningResetInfo {
            approval_url,
            session,
        }) => AuthChallenge {
            session: session.clone(),
            flows: vec![vec![OAUTH_STAGE.to_owned()]],
            completed: Vec::new(),
            oauth_url: Some(approval_url.to_string()),
            homeserver: client.homeserver().to_string(),
            refusal: None,
        },
    }
}

async fn identity_replaced(client: &Client, unpublished: &Unpublished) {
    tracing::info!("published the account's new cross-signing identity");
    unpublished.forget();
    if let Ok(own) = own_user(client) {
        read_own_identity_back(client, &own).await;
    }
}

async fn set_up_identity(
    client: &Client,
    unpublished: &Unpublished,
    auth: Option<&AuthAnswer>,
) -> Result<IdentityOutcome> {
    let own = own_user(client)?;
    if homeserver_holds_identity(client, &own).await? {
        unpublished.forget();
        read_own_identity_back(client, &own).await;
        return Ok(IdentityOutcome::Ready);
    }
    tracing::info!("the account has no published cross-signing identity; creating one");
    let outcome = create_identity(client, auth).await;
    if matches!(outcome, Ok(IdentityOutcome::Ready)) {
        tracing::info!("created and published the account's cross-signing identity");
        unpublished.forget();
        read_own_identity_back(client, &own).await;
    } else {
        unpublished.remember(client, &own).await;
    }
    outcome
}

async fn homeserver_holds_identity(client: &Client, own: &UserId) -> Result<bool> {
    let mut request = get_keys::v3::Request::new();
    request.device_keys = BTreeMap::from([(own.to_owned(), Vec::new())]);
    let response = client
        .send(request)
        .await
        .map_err(|e| AppError::Other(format!("could not query the account's identity: {e}")))?;
    Ok(response.master_keys.contains_key(own))
}

async fn create_identity(client: &Client, auth: Option<&AuthAnswer>) -> Result<IdentityOutcome> {
    let auth_data = uia::answered(client, auth)?;
    let Err(error) = client.encryption().bootstrap_cross_signing(auth_data).await else {
        return Ok(IdentityOutcome::Ready);
    };
    let Some(info) = error.as_uiaa_response() else {
        return Err(AppError::Other(format!(
            "could not publish cross-signing keys: {error}"
        )));
    };
    tracing::warn!(
        "the homeserver wants the user to authenticate before it takes this account's first cross-signing keys"
    );
    Ok(IdentityOutcome::NeedsAuthentication(Box::new(
        uia::challenge(client, info),
    )))
}

async fn read_own_identity_back(client: &Client, own: &UserId) {
    if let Err(e) = client.encryption().request_user_identity(own).await {
        tracing::debug!("could not read the account's identity back: {e}");
    }
}

fn failed_recovery(error: &RecoveryError) -> Result<RecoveryAttempt> {
    match error {
        RecoveryError::SecretStorage(SecretStorageError::SecretStorageKey(decode))
            if is_wrong_key(decode) =>
        {
            Ok(RecoveryAttempt::WrongKey)
        }
        RecoveryError::SecretStorage(
            SecretStorageError::InconsistentBackupDecryptionKey
            | SecretStorageError::MissingOrInvalidBackupDecryptionKey
            | SecretStorageError::ImportError {
                name: SecretName::RecoveryKey,
                ..
            },
        ) => {
            tracing::warn!(
                "the recovery key unlocked this session but not the key backup: {error}"
            );
            Ok(RecoveryAttempt::UnlockedWithoutBackup)
        }
        _ => Err(AppError::Other(format!(
            "could not use the recovery key: {error}"
        ))),
    }
}

fn is_wrong_key(error: &DecodeError) -> bool {
    matches!(
        error,
        DecodeError::Mac(_)
            | DecodeError::Prefix(..)
            | DecodeError::Parity(..)
            | DecodeError::Base58(_)
            | DecodeError::Base64(_)
            | DecodeError::KeyLength(..)
    )
}

type Wakes = Pin<Box<dyn Stream<Item = Wake> + Send>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Wake {
    Reassess,
    Ignore,
}

fn wake_if(concerns_us: bool) -> Wake {
    if concerns_us {
        Wake::Reassess
    } else {
        Wake::Ignore
    }
}

pub(super) fn names<V, W>(
    new: &BTreeMap<OwnedUserId, V>,
    changed: &BTreeMap<OwnedUserId, W>,
    user: &UserId,
) -> bool {
    new.contains_key(user) || changed.contains_key(user)
}

async fn wakes(client: &Client, own: &OwnedUserId) -> Result<SelectAll<Wakes>> {
    let encryption = client.encryption();
    let identity_owner = own.clone();
    let identities = encryption
        .user_identities_stream()
        .await
        .map_err(AppError::other)?
        .map(move |update| wake_if(names(&update.new, &update.changed, &identity_owner)));
    let device_owner = own.clone();
    let devices = encryption
        .devices_stream()
        .await
        .map_err(AppError::other)?
        .map(move |update| wake_if(names(&update.new, &update.changed, &device_owner)));
    let streams: Vec<Wakes> = vec![
        Box::pin(identities),
        Box::pin(devices),
        Box::pin(encryption.verification_state().map(|_| Wake::Reassess)),
        Box::pin(encryption.recovery().state_stream().map(|_| Wake::Reassess)),
        Box::pin(encryption.backups().state_stream().map(|_| Wake::Reassess)),
    ];
    Ok(select_all(streams))
}

async fn next_change(wakes: &mut SelectAll<Wakes>, changed: &Notify) -> bool {
    loop {
        tokio::select! {
            wake = wakes.next() => match wake {
                Some(Wake::Reassess) => return true,
                Some(Wake::Ignore) => {}
                None => return false,
            },
            () = changed.notified() => return true,
        }
    }
}

async fn follow(
    client: &Client,
    changed: &Notify,
    unpublished: &Unpublished,
    state_tx: mpsc::Sender<AccountEncryption>,
) -> Result<()> {
    let own = own_user(client)?;
    let mut wakes = wakes(client, &own).await?;
    let mut reported = None;
    loop {
        match assess(client, &own, unpublished).await {
            Ok(state) if reported != Some(state) => {
                tracing::info!(?state, "assessed this session's encryption");
                if state_tx.send(state).await.is_err() {
                    return Ok(());
                }
                reported = Some(state);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("could not assess this session's encryption: {e}"),
        }
        if !next_change(&mut wakes, changed).await {
            return Ok(());
        }
    }
}

async fn assess(
    client: &Client,
    own: &UserId,
    unpublished: &Unpublished,
) -> Result<AccountEncryption> {
    let encryption = client.encryption();
    let identity = encryption
        .get_user_identity(own)
        .await
        .map_err(AppError::other)?
        .filter(|identity| !unpublished.holds(identity));
    let trust = match identity {
        None => SessionTrust::NoIdentity,
        Some(identity) if identity.is_verified() && own_device_signed(client).await? => {
            SessionTrust::Verified
        }
        Some(_) => SessionTrust::Unverified,
    };
    let verified_sessions = trust == SessionTrust::Unverified
        && encryption
            .has_devices_to_verify_against()
            .await
            .unwrap_or_else(|e| {
                tracing::debug!("could not list the sessions to verify against: {e}");
                false
            });
    let recovery = recovery_of(encryption.recovery().state());
    let key_storage = if recovery == Recovery::Off {
        key_storage(client).await
    } else {
        KeyStorage::Allowed
    };
    Ok(AccountEncryption {
        trust,
        verified_sessions,
        recovery,
        key_storage,
    })
}

async fn own_device_signed(client: &Client) -> Result<bool> {
    Ok(client
        .encryption()
        .get_own_device()
        .await
        .map_err(AppError::other)?
        .is_some_and(|device| device.is_cross_signed_by_owner()))
}

fn recovery_of(state: RecoveryState) -> Recovery {
    match state {
        RecoveryState::Unknown => Recovery::Unknown,
        RecoveryState::Disabled => Recovery::Off,
        RecoveryState::Enabled => Recovery::On,
        RecoveryState::Incomplete => Recovery::Incomplete,
    }
}

async fn key_storage(client: &Client) -> KeyStorage {
    let turned_off = match preference(client, KEY_BACKUP_PREFERENCE, "enabled").await {
        Some(enabled) => !enabled,
        None => preference(client, UNSTABLE_BACKUP_DISABLED, "disabled")
            .await
            .unwrap_or(false),
    };
    if turned_off {
        KeyStorage::TurnedOff
    } else {
        KeyStorage::Allowed
    }
}

async fn preference(client: &Client, event_type: &str, field: &str) -> Option<bool> {
    let fetched = client
        .account()
        .fetch_account_data(GlobalAccountDataEventType::from(event_type))
        .await;
    match fetched {
        Ok(Some(raw)) => raw.get_field::<bool>(field).ok().flatten(),
        Ok(None) => None,
        Err(e) => {
            tracing::debug!(event_type, "could not read the key storage preference: {e}");
            None
        }
    }
}
