mod auth;
mod media;
mod preview;
mod profile;
mod rooms;
mod timeline;
mod verification;

use std::collections::HashMap;
use std::path::PathBuf;
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
use matrix_sdk::ruma::{IdParseError, OwnedEventId, OwnedRoomId};
use matrix_sdk::utils::local_server::LocalServerRedirectHandle;
use tokio::fs;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio_util::sync::CancellationToken;

use self::media::MediaService;
use self::profile::PronounCache;
use crate::domain::models::{
    LoginCredentials, OAuthLoginData, RoomId, ServerInfo, Session, SyncEvent, TimelineCommand,
    TimelineUpdate, VerificationEvent,
};
use crate::error::{AppError, Result};
use crate::ports::matrix::{
    AuthPort, AuthenticatedSession, MediaPort, SessionPort, SyncPort, TimelinePort,
    VerificationPort,
};
use crate::ports::media::MediaCache;

pub struct MatrixAdapter {
    data_dir: PathBuf,
    cache_dir: PathBuf,
    client: RwLock<Option<Client>>,
    redirect_handle: Mutex<Option<LocalServerRedirectHandle>>,
    media: Arc<MediaService>,
}

impl MatrixAdapter {
    pub fn new(data_dir: PathBuf, cache_dir: PathBuf) -> Self {
        let media = MediaService::new(&cache_dir);
        Self {
            data_dir,
            cache_dir,
            client: RwLock::new(None),
            redirect_handle: Mutex::new(None),
            media,
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

    fn authenticate(&self, client: Client, session: Session) -> AuthenticatedSession {
        let authed = Arc::new(AuthedMatrix {
            client,
            data_dir: self.data_dir.clone(),
            cache_dir: self.cache_dir.clone(),
            media: Arc::clone(&self.media),
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
        AuthenticatedSession {
            session,
            sync,
            timeline,
            media,
            verification,
            lifecycle: authed,
        }
    }
}

#[async_trait]
impl AuthPort for MatrixAdapter {
    async fn discover_auth(&self, homeserver: &str, passphrase: &str) -> Result<ServerInfo> {
        auth::discover_auth(
            &self.client,
            &self.data_dir,
            &self.cache_dir,
            homeserver,
            passphrase,
        )
        .await
    }

    async fn login_password(&self, creds: LoginCredentials) -> Result<AuthenticatedSession> {
        let client = self.get_client().await?;
        let session = auth::login_password(&client, creds).await?;
        drop(self.client.write().await.take());
        Ok(self.authenticate(client, session))
    }

    async fn login_oauth_start(&self) -> Result<OAuthLoginData> {
        let client = self.get_client().await?;
        auth::login_oauth_start(&client, &self.redirect_handle).await
    }

    async fn login_oauth_finish(&self) -> Result<AuthenticatedSession> {
        let client = self.get_client().await?;
        let session = auth::login_oauth_finish(&client, &self.redirect_handle).await?;
        drop(self.client.write().await.take());
        Ok(self.authenticate(client, session))
    }

    async fn cancel_oauth(&self) {
        let pending = self.redirect_handle.lock().await.take();
        if pending.is_some() {
            tracing::debug!("shutting down pending OAuth redirect server");
        }
    }

    async fn restore_session(
        &self,
        session: &Session,
        passphrase: &str,
        on_progress: Box<dyn Fn(String) + Send + Sync>,
    ) -> Result<AuthenticatedSession> {
        auth::restore_session(
            &self.client,
            &self.data_dir,
            &self.cache_dir,
            session,
            passphrase,
            on_progress,
        )
        .await?;
        let client = self.get_client().await?;
        drop(self.client.write().await.take());
        Ok(self.authenticate(client, session.clone()))
    }
}

struct AuthedMatrix {
    client: Client,
    data_dir: PathBuf,
    cache_dir: PathBuf,
    media: Arc<MediaService>,
    media_sources: Arc<StdMutex<HashMap<String, MediaSource>>>,
    pronouns: Arc<PronounCache>,
    verification_request: Mutex<Option<VerificationRequest>>,
    sas_verification: Mutex<Option<SasVerification>>,
    verification_req_rx: Mutex<Option<mpsc::Receiver<VerificationRequest>>>,
    verification_handler_guards: Mutex<Vec<EventHandlerDropGuard>>,
}

impl AuthedMatrix {
    fn clear_media_sources(&self) {
        if let Ok(mut sources) = self.media_sources.lock() {
            sources.clear();
        }
    }

    fn room(&self, room_id: &RoomId) -> Result<matrix_sdk::Room> {
        let room_id_parsed: OwnedRoomId = room_id
            .as_ref()
            .try_into()
            .map_err(|e: IdParseError| AppError::Other(e.to_string()))?;
        self.client
            .get_room(&room_id_parsed)
            .ok_or_else(|| AppError::Other("Room not found".into()))
    }
}

#[async_trait]
impl SyncPort for AuthedMatrix {
    async fn start_sync(
        &self,
        on_sync: Box<dyn Fn(SyncEvent) + Send + Sync>,
        cancel: CancellationToken,
    ) -> Result<()> {
        tracing::info!("starting continuous sync loop");
        rooms::start_sync(
            &self.client,
            Arc::clone(&self.media),
            on_sync.into(),
            cancel,
        )
        .await
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
            &self.client,
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
        let room = self.room(room_id)?;
        let content = RoomMessageEventContent::text_plain(body);
        room.send(content)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        Ok(())
    }

    async fn send_reply(&self, room_id: &RoomId, body: &str, in_reply_to: &str) -> Result<()> {
        let room = self.room(room_id)?;
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
            .download_media(&self.client, &self.media_sources, event_id, thumbnail)
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
            &self.client,
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
        auth::subscribe_session_changes(&self.client, session_tx).await
    }

    async fn fetch_user_avatar(&self) -> Result<Option<PathBuf>> {
        Ok(self.media.fetch_user_avatar(&self.client).await)
    }

    async fn logout(&self) -> Result<()> {
        tracing::info!("logging out");
        self.media.clear().await;
        self.clear_media_sources();
        self.verification_handler_guards.lock().await.clear();
        *self.verification_req_rx.lock().await = None;
        if let Err(e) = self.client.logout().await {
            tracing::warn!("failed to logout from server: {e}");
        }
        Ok(())
    }

    async fn clear_store(&self) -> Result<()> {
        tracing::info!("clearing matrix store");
        self.media.clear().await;
        self.clear_media_sources();
        self.verification_handler_guards.lock().await.clear();
        *self.verification_req_rx.lock().await = None;
        let store_path = self.data_dir.join("matrix-store");
        if store_path.exists() {
            fs::remove_dir_all(&store_path).await?;
        }
        let cache_path = self.cache_dir.join("matrix-store");
        if cache_path.exists() {
            fs::remove_dir_all(&cache_path).await?;
        }
        tracing::debug!("matrix store cleared");
        Ok(())
    }
}
