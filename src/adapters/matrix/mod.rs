mod auth;
mod identity;
mod media;
mod preview;
mod profile;
mod rooms;
mod store;
mod timeline;
mod verification;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use matrix_sdk::Client;
use matrix_sdk::encryption::verification::{SasVerification, VerificationRequest};
use matrix_sdk::event_handler::EventHandlerDropGuard;
use matrix_sdk::room::reply::{EnforceThread, Reply};
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::ruma::events::room::message::{
    AddMentions, RoomMessageEventContent, RoomMessageEventContentWithoutRelation,
};
use matrix_sdk::ruma::events::space_order::SpaceOrderEventContent;
use matrix_sdk::ruma::{IdParseError, OwnedEventId, OwnedRoomId, SpaceChildOrder};
use matrix_sdk::utils::local_server::LocalServerRedirectHandle;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio_util::sync::CancellationToken;

use self::media::MediaService;
use self::profile::PronounCache;
use self::store::{AdoptedStore, StoreLayout, StorePaths};
use crate::domain::account::AccountScope;
use crate::domain::models::{
    LoginCredentials, OAuthLoginData, RoomId, ServerInfo, Session, SyncOutcome, TimelineCommand,
    TimelineUpdate, VerificationEvent,
};
use crate::error::{AppError, Result};
use crate::ports::matrix::{
    AuthPort, AuthenticatedSession, CleanupReport, MediaPort, ProgressSink, SessionPort,
    SpaceOrderPort, StoreAdoption, SyncPort, SyncSink, TimelinePort, VerificationPort,
};
use crate::ports::media::MediaCache;

pub struct MatrixAdapter {
    layout: StoreLayout,
    client: RwLock<Option<Client>>,
    pending_store: Mutex<Option<StorePaths>>,
    redirect_handle: Mutex<Option<LocalServerRedirectHandle>>,
    media: Arc<MediaService>,
    swept: AtomicBool,
}

impl MatrixAdapter {
    pub fn new(data_dir: PathBuf, cache_dir: PathBuf) -> Self {
        let media = MediaService::new(&cache_dir);
        Self {
            layout: StoreLayout::new(data_dir, cache_dir),
            client: RwLock::new(None),
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

    async fn abandon_adoption(&self, adopted: AdoptedStore) {
        let report = self.layout.roll_back_adoption(adopted).await;
        if !report.is_clean() {
            tracing::warn!(
                "previous store not restored after a failed adoption: {}",
                report.summary()
            );
        }
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

async fn authenticate(
    layout: StoreLayout,
    media: Arc<MediaService>,
    client: Client,
    session: Session,
    account: AccountScope,
) -> AuthenticatedSession {
    media.open(&account).await;
    let authed = Arc::new(AuthedMatrix {
        client: RwLock::new(Some(client)),
        layout,
        account,
        media,
        media_sources: Arc::new(StdMutex::new(HashMap::new())),
        pronouns: Arc::new(PronounCache::default()),
        verification_request: Mutex::new(None),
        sas_verification: Mutex::new(None),
        verification_req_rx: Mutex::new(None),
        verification_handler_guards: Mutex::new(Vec::new()),
    });
    let sync = Arc::clone(&authed);
    let timeline = Arc::clone(&authed);
    let media = Arc::clone(&authed);
    let verification = Arc::clone(&authed);
    let space_order = Arc::clone(&authed);
    AuthenticatedSession {
        session,
        sync,
        timeline,
        media,
        verification,
        space_order,
        lifecycle: authed,
    }
}

struct UncommittedAdoption {
    layout: StoreLayout,
    media: Arc<MediaService>,
    adopted: AdoptedStore,
    client: Client,
    session: Session,
    account: AccountScope,
}

#[async_trait]
impl StoreAdoption for UncommittedAdoption {
    async fn commit(self: Box<Self>) -> AuthenticatedSession {
        let Self {
            layout,
            media,
            adopted,
            client,
            session,
            account,
        } = *self;
        layout.commit_adoption(adopted).await;
        authenticate(layout, media, client, session, account).await
    }

    async fn roll_back(self: Box<Self>) -> CleanupReport {
        let Self {
            layout,
            adopted,
            client,
            ..
        } = *self;
        drop(client);
        layout.roll_back_adoption(adopted).await
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
        auth::login_oauth_start(&client, &self.redirect_handle).await
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
                self.abandon_adoption(adopted).await;
                return Err(e);
            }
        };

        Ok(Box::new(UncommittedAdoption {
            layout: self.layout.clone(),
            media: Arc::clone(&self.media),
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
}

struct AuthedMatrix {
    client: RwLock<Option<Client>>,
    layout: StoreLayout,
    account: AccountScope,
    media: Arc<MediaService>,
    media_sources: Arc<StdMutex<HashMap<String, MediaSource>>>,
    pronouns: Arc<PronounCache>,
    verification_request: Mutex<Option<VerificationRequest>>,
    sas_verification: Mutex<Option<SasVerification>>,
    verification_req_rx: Mutex<Option<mpsc::Receiver<VerificationRequest>>>,
    verification_handler_guards: Mutex<Vec<EventHandlerDropGuard>>,
}

impl AuthedMatrix {
    async fn client(&self) -> Result<Client> {
        self.client
            .read()
            .await
            .clone()
            .ok_or_else(|| AppError::Other("The session has been closed".into()))
    }

    fn clear_media_sources(&self) {
        if let Ok(mut sources) = self.media_sources.lock() {
            sources.clear();
        }
    }

    async fn room(&self, room_id: &RoomId) -> Result<matrix_sdk::Room> {
        let room_id_parsed: OwnedRoomId = room_id
            .as_ref()
            .try_into()
            .map_err(|e: IdParseError| AppError::Other(e.to_string()))?;
        self.client()
            .await?
            .get_room(&room_id_parsed)
            .ok_or_else(|| AppError::Other("Room not found".into()))
    }

    async fn release_session_resources(&self) {
        self.clear_media_sources();
        self.verification_handler_guards.lock().await.clear();
        *self.verification_req_rx.lock().await = None;
    }
}

#[async_trait]
impl SyncPort for AuthedMatrix {
    async fn start_sync(&self, on_sync: SyncSink, cancel: CancellationToken) -> SyncOutcome {
        tracing::info!("starting continuous sync loop");
        let client = match self.client().await {
            Ok(client) => client,
            Err(e) => return SyncOutcome::Fatal(e.to_string()),
        };
        rooms::start_sync(&client, Arc::clone(&self.media), on_sync, cancel).await
    }
}

#[async_trait]
impl SpaceOrderPort for AuthedMatrix {
    async fn set_space_order(&self, space_id: &RoomId, order: &str) -> Result<()> {
        let room = self.room(space_id).await?;
        let order = SpaceChildOrder::parse(order).map_err(|e| AppError::Other(e.to_string()))?;
        room.set_account_data(SpaceOrderEventContent::new(order))
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl TimelinePort for AuthedMatrix {
    async fn subscribe_timeline(
        &self,
        room_id: &RoomId,
        timeline_tx: mpsc::Sender<TimelineUpdate>,
        cmd_rx: mpsc::UnboundedReceiver<TimelineCommand>,
    ) -> Result<()> {
        tracing::info!(%room_id, "subscribing to timeline");
        timeline::subscribe_timeline(
            &self.client().await?,
            &self.media,
            &self.media_sources,
            &self.pronouns,
            room_id,
            timeline_tx,
            cmd_rx,
        )
        .await
    }

    async fn send_text(&self, room_id: &RoomId, body: &str) -> Result<()> {
        let room = self.room(room_id).await?;
        let content = RoomMessageEventContent::text_plain(body);
        room.send(content)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        Ok(())
    }

    async fn send_reply(&self, room_id: &RoomId, body: &str, in_reply_to: &str) -> Result<()> {
        let room = self.room(room_id).await?;
        let event_id: OwnedEventId = in_reply_to
            .try_into()
            .map_err(|e: IdParseError| AppError::Other(e.to_string()))?;
        let content = RoomMessageEventContentWithoutRelation::text_plain(body);
        let reply = Reply {
            event_id,
            enforce_thread: EnforceThread::MaybeThreaded,
            add_mentions: AddMentions::Yes,
        };
        let content = room
            .make_reply_event(content, reply)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        room.send(content)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl MediaPort for AuthedMatrix {
    async fn download_media(&self, event_id: &str, thumbnail: bool) -> Result<Vec<u8>> {
        self.media
            .download_media(
                &self.client().await?,
                &self.media_sources,
                event_id,
                thumbnail,
            )
            .await
    }
}

#[async_trait]
impl VerificationPort for AuthedMatrix {
    async fn listen_for_verification(
        &self,
        verification_tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Result<()> {
        verification::listen_for_verification(
            &self.client().await?,
            &self.verification_req_rx,
            &self.verification_handler_guards,
            &self.verification_request,
            &self.sas_verification,
            verification_tx,
        )
        .await
    }

    async fn accept_verification(&self) -> Result<()> {
        verification::accept_verification(&self.verification_request).await
    }

    async fn confirm_verification(&self) -> Result<()> {
        verification::confirm_verification(&self.sas_verification).await
    }

    async fn reject_verification(&self) -> Result<()> {
        verification::reject_verification(&self.sas_verification, &self.verification_request).await
    }
}

#[async_trait]
impl SessionPort for AuthedMatrix {
    async fn subscribe_session_changes(
        &self,
        session_tx: mpsc::UnboundedSender<Session>,
    ) -> Result<()> {
        auth::subscribe_session_changes(&self.client().await?, session_tx).await
    }

    async fn fetch_user_avatar(&self) -> Result<Option<PathBuf>> {
        Ok(self.media.fetch_user_avatar(&self.client().await?).await)
    }

    async fn logout(&self) -> Result<()> {
        tracing::info!("logging out");
        self.release_session_resources().await;
        if let Err(e) = self.client().await?.logout().await {
            tracing::warn!("failed to logout from server: {e}");
        }
        Ok(())
    }

    async fn clear_store(&self) -> CleanupReport {
        tracing::info!("clearing local account data");
        self.release_session_resources().await;

        let mut report = self.media.close(&self.account).await;

        drop(self.client.write().await.take());
        report.merge(self.layout.purge_account(&self.account).await);

        if report.is_clean() {
            tracing::info!("local account data cleared");
        } else {
            tracing::warn!("local account data not fully cleared: {}", report.summary());
        }
        report
    }
}
