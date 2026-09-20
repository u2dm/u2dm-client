mod attachment;
mod auth;
mod identity;
mod journal;
mod media;
mod preview;
mod profile;
mod rooms;
mod session;
mod stickers;
mod store;
mod timeline;
mod verification;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use matrix_sdk::Client;
use matrix_sdk::ruma::OwnedDeviceId;
use matrix_sdk::utils::local_server::LocalServerRedirectHandle;
use tokio::sync::{Mutex, RwLock};

use self::media::MediaService;
use self::session::authenticate;
use self::store::{AdoptedStore, StoreLayout, StorePaths};
use crate::adapters::instance::InstanceClaim;
use crate::domain::account::AccountScope;
use crate::domain::auth::{LoginCredentials, OAuthLoginData, ServerInfo, Session};
use crate::error::{AppError, Result};
use crate::ports::matrix::{
    AuthPort, AuthenticatedSession, CleanupReport, InterruptedLogin, LocalDataOwnership,
    ProgressSink, StoreAdoption,
};
use crate::ports::media::MediaCache;

pub struct MatrixAdapter {
    instance: InstanceClaim,
    layout: StoreLayout,
    client: RwLock<Option<Client>>,
    reauth_client: RwLock<Option<Client>>,
    pending_store: Mutex<Option<StorePaths>>,
    redirect_handle: Mutex<Option<LocalServerRedirectHandle>>,
    media: Arc<MediaService>,
    swept: AtomicBool,
}

impl MatrixAdapter {
    pub fn new(data_dir: PathBuf, cache_dir: PathBuf) -> Self {
        let instance = InstanceClaim::take(&data_dir);
        let media = MediaService::new(&cache_dir);
        Self {
            instance,
            layout: StoreLayout::new(data_dir, cache_dir),
            client: RwLock::new(None),
            reauth_client: RwLock::new(None),
            pending_store: Mutex::new(None),
            redirect_handle: Mutex::new(None),
            media,
            swept: AtomicBool::new(false),
        }
    }

    async fn get_client(&self) -> Result<Client> {
        self.client
            .read()
            .await
            .clone()
            .ok_or_else(|| AppError::Other("No client, run server discovery first".into()))
    }

    pub fn media_cache(&self) -> Arc<dyn MediaCache> {
        Arc::new(media::MaterializedMedia::new(Arc::clone(&self.media)))
    }

    async fn sweep_stale_once(&self, keep: Option<&AccountScope>) {
        if self.swept.swap(true, Ordering::Relaxed) {
            return;
        }
        self.layout.sweep_stale().await;
        self.media.sweep(keep).await;
    }

    async fn discard_pending_store(&self) {
        drop(self.client.write().await.take());
        let Some(paths) = self.pending_store.lock().await.take() else {
            return;
        };
        self.purge_login_scratch(&paths).await;
    }

    async fn discard_reauth_client(&self) {
        drop(self.reauth_client.write().await.take());
    }

    async fn open_reauth_client(&self, prior: &Session, passphrase: &str) -> Result<Client> {
        self.discard_reauth_client().await;
        let paths = self.layout.account(&AccountScope::from_session(prior));
        auth::open_account_store(&paths, prior, passphrase).await
    }

    async fn resume_on_preserved_store(
        &self,
        client: Client,
        prior: &Session,
        session: Session,
    ) -> Result<AuthenticatedSession> {
        identity::ensure_identity_matches_server(&client).await?;
        let account = AccountScope::from_session(prior);
        Ok(self.authenticate(client, session, account).await)
    }

    async fn purge_login_scratch(&self, paths: &StorePaths) {
        let report = self.layout.purge(paths).await;
        if !report.is_clean() {
            tracing::warn!(
                "login scratch store not fully removed: {}",
                report.summary()
            );
        }
    }

    async fn authenticate(
        &self,
        client: Client,
        session: Session,
        account: AccountScope,
    ) -> AuthenticatedSession {
        authenticate(
            self.layout.clone(),
            Arc::clone(&self.media),
            client,
            session,
            account,
        )
        .await
    }
}

struct UncommittedAdoption {
    layout: StoreLayout,
    media: Arc<MediaService>,
    txn: String,
    adopted: AdoptedStore,
    client: Client,
    session: Session,
    account: AccountScope,
}

#[async_trait]
impl StoreAdoption for UncommittedAdoption {
    fn transaction(&self) -> &str {
        &self.txn
    }

    async fn credentials_staged(&self) -> Result<()> {
        self.adopted.credentials_staged().await
    }

    async fn credentials_written(&self) -> Result<()> {
        self.adopted.credentials_written().await
    }

    async fn rolling_back(&self) -> Result<()> {
        self.adopted.rolling_back().await
    }

    async fn commit(self: Box<Self>) -> AuthenticatedSession {
        let Self {
            layout,
            media,
            adopted,
            client,
            session,
            account,
            ..
        } = *self;
        layout.commit_adoption(adopted).await;
        authenticate(layout, media, client, session, account).await
    }

    async fn unwind(self: Box<Self>) -> CleanupReport {
        let Self {
            layout,
            adopted,
            client,
            ..
        } = *self;
        drop(client);
        layout.unwind_adoption(adopted).await
    }
}

#[async_trait]
impl AuthPort for MatrixAdapter {
    async fn discover_auth(&self, homeserver: &str, passphrase: &str) -> Result<ServerInfo> {
        self.sweep_stale_once(None).await;
        self.discard_pending_store().await;

        let paths = self.layout.pending();
        let (client, info) = match auth::discover_auth(&paths, homeserver, passphrase).await {
            Ok(discovered) => discovered,
            Err(e) => {
                self.purge_login_scratch(&paths).await;
                return Err(e);
            }
        };

        *self.pending_store.lock().await = Some(paths);
        *self.client.write().await = Some(client);
        Ok(info)
    }

    async fn login_password(&self, creds: LoginCredentials) -> Result<Session> {
        let client = self.get_client().await?;
        auth::login_password(&client, creds).await
    }

    async fn login_oauth_start(&self) -> Result<OAuthLoginData> {
        let client = self.get_client().await?;
        auth::login_oauth_start(&client, &self.redirect_handle, None).await
    }

    async fn login_oauth_finish(&self) -> Result<Session> {
        let client = self.get_client().await?;
        auth::login_oauth_finish(&client, &self.redirect_handle).await
    }

    async fn cancel_oauth(&self) {
        let pending = self.redirect_handle.lock().await.take();
        if pending.is_some() {
            tracing::debug!("shutting down pending OAuth redirect server");
        }
        self.discard_reauth_client().await;
    }

    async fn adopt_session(
        &self,
        session: &Session,
        passphrase: &str,
    ) -> Result<Box<dyn StoreAdoption>> {
        let account = AccountScope::from_session(session);

        drop(self.client.write().await.take());
        let pending = self.pending_store.lock().await.take().ok_or_else(|| {
            AppError::Other("No login store to adopt, run server discovery first".into())
        })?;

        let adopted = match self.layout.adopt(&pending, &account).await {
            Ok(adopted) => adopted,
            Err(e) => {
                self.purge_login_scratch(&pending).await;
                return Err(e);
            }
        };

        let client = match auth::open_session(&adopted.paths, session, passphrase, &|_| {}).await {
            Ok(client) => client,
            Err(e) => {
                self.layout.abandon_adoption(adopted).await;
                return Err(e);
            }
        };

        Ok(Box::new(UncommittedAdoption {
            layout: self.layout.clone(),
            media: Arc::clone(&self.media),
            txn: adopted.txn.clone(),
            adopted,
            client,
            session: session.clone(),
            account,
        }))
    }

    async fn restore_session(
        &self,
        session: &Session,
        passphrase: &str,
        on_progress: ProgressSink,
    ) -> Result<AuthenticatedSession> {
        let account = AccountScope::from_session(session);
        self.sweep_stale_once(Some(&account)).await;

        let paths = self.layout.account(&account);
        let client = auth::open_session(&paths, session, passphrase, on_progress.as_ref()).await?;
        Ok(self.authenticate(client, session.clone(), account).await)
    }

    async fn reauthenticate(
        &self,
        prior: &Session,
        passphrase: &str,
        creds: LoginCredentials,
    ) -> Result<AuthenticatedSession> {
        let client = self.open_reauth_client(prior, passphrase).await?;
        let session = auth::reauth_password(&client, prior, creds).await?;
        self.resume_on_preserved_store(client, prior, session).await
    }

    async fn reauth_oauth_start(
        &self,
        prior: &Session,
        passphrase: &str,
    ) -> Result<OAuthLoginData> {
        let client = self.open_reauth_client(prior, passphrase).await?;
        let device_id: OwnedDeviceId = prior.device_id.as_str().into();
        let data = auth::login_oauth_start(&client, &self.redirect_handle, Some(device_id)).await?;
        *self.reauth_client.write().await = Some(client);
        Ok(data)
    }

    async fn reauth_oauth_finish(&self, prior: &Session) -> Result<AuthenticatedSession> {
        let client = self
            .reauth_client
            .write()
            .await
            .take()
            .ok_or_else(|| AppError::Other("No re-authentication is in progress".into()))?;
        let session = auth::reauth_oauth_finish(&client, &self.redirect_handle, prior).await?;
        self.resume_on_preserved_store(client, prior, session).await
    }

    fn local_data_ownership(&self) -> LocalDataOwnership {
        self.instance.ownership()
    }

    async fn interrupted_logins(&self) -> Result<Vec<InterruptedLogin>> {
        self.layout.interrupted_logins().await
    }

    async fn unwind_login(&self, txn: &str) -> CleanupReport {
        self.layout.unwind_login(txn).await
    }

    async fn settle_login(&self, txn: &str) -> CleanupReport {
        self.layout.settle_login(txn).await
    }

    async fn mark_rolled_back(&self, txn: &str) -> Result<()> {
        self.layout.mark_rolled_back(txn).await
    }

    async fn forget_login(&self, txn: &str) {
        self.layout.forget_login(txn).await;
    }
}
