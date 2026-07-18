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
use tokio::sync::{Mutex, RwLock, mpsc};

use self::media::MediaService;
use self::profile::PronounCache;
use crate::domain::models::{
    LoginCredentials, OAuthLoginData, RoomId, ServerInfo, Session, SyncEvent, TimelineCommand,
    TimelineUpdate, VerificationEvent,
};
use crate::error::{AppError, Result};
use crate::ports::matrix::MatrixPort;
use crate::ports::media::MediaCache;

pub struct MatrixAdapter {
    data_dir: PathBuf,
    cache_dir: PathBuf,
    client: RwLock<Option<Client>>,
    redirect_handle: Mutex<Option<LocalServerRedirectHandle>>,
    verification_request: Mutex<Option<VerificationRequest>>,
    sas_verification: Mutex<Option<SasVerification>>,
    media_sources: Arc<StdMutex<HashMap<String, MediaSource>>>,
    media: Arc<MediaService>,
    pronouns: Arc<PronounCache>,
    verification_req_rx: Mutex<Option<mpsc::UnboundedReceiver<VerificationRequest>>>,
    verification_handler_guards: Mutex<Vec<EventHandlerDropGuard>>,
}

impl MatrixAdapter {
    pub fn new(data_dir: PathBuf, cache_dir: PathBuf) -> Self {
        let media = MediaService::new(&cache_dir);
        Self {
            data_dir,
            cache_dir,
            client: RwLock::new(None),
            redirect_handle: Mutex::new(None),
            verification_request: Mutex::new(None),
            sas_verification: Mutex::new(None),
            media_sources: Arc::new(StdMutex::new(HashMap::new())),
            media,
            pronouns: Arc::new(PronounCache::default()),
            verification_req_rx: Mutex::new(None),
            verification_handler_guards: Mutex::new(Vec::new()),
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
}

#[async_trait]
impl MatrixPort for MatrixAdapter {
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

    async fn subscribe_timeline(
        &self,
        room_id: &RoomId,
        timeline_tx: mpsc::UnboundedSender<TimelineUpdate>,
        cmd_rx: mpsc::UnboundedReceiver<TimelineCommand>,
    ) -> Result<()> {
        tracing::info!(%room_id, "subscribing to timeline");
        let client = self.get_client().await?;
        timeline::subscribe_timeline(
            &client,
            &self.media,
            &self.media_sources,
            &self.pronouns,
            room_id,
            timeline_tx,
            cmd_rx,
        )
        .await
    }

    async fn start_sync(&self, on_sync: Box<dyn Fn(SyncEvent) + Send + Sync>) -> Result<()> {
        tracing::info!("starting continuous sync loop");
        let client = self.get_client().await?;
        rooms::start_sync(&client, Arc::clone(&self.media), on_sync.into()).await
    }

    async fn restore_session(
        &self,
        session: &Session,
        passphrase: &str,
        on_progress: Box<dyn Fn(String) + Send + Sync>,
    ) -> Result<()> {
        auth::restore_session(
            &self.client,
            &self.data_dir,
            &self.cache_dir,
            session,
            passphrase,
            on_progress,
        )
        .await
    }

    async fn logout(&self) -> Result<()> {
        self.media.clear().await;
        auth::logout(
            &self.client,
            &self.verification_req_rx,
            &self.verification_handler_guards,
        )
        .await
    }

    async fn clear_store(&self) -> Result<()> {
        self.media.clear().await;
        auth::clear_store(
            &self.client,
            &self.data_dir,
            &self.cache_dir,
            &self.verification_req_rx,
            &self.verification_handler_guards,
        )
        .await
    }

    async fn listen_for_verification(
        &self,
        verification_tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Result<()> {
        let client = self.get_client().await?;
        verification::listen_for_verification(
            &client,
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

    async fn send_text(&self, room_id: &RoomId, body: &str) -> Result<()> {
        let client = self.get_client().await?;
        let room_id_parsed: OwnedRoomId = room_id
            .as_ref()
            .try_into()
            .map_err(|e: IdParseError| AppError::Other(e.to_string()))?;
        let room = client
            .get_room(&room_id_parsed)
            .ok_or_else(|| AppError::Other("Room not found".into()))?;
        let content = RoomMessageEventContent::text_plain(body);
        room.send(content)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        Ok(())
    }

    async fn send_reply(&self, room_id: &RoomId, body: &str, in_reply_to: &str) -> Result<()> {
        let client = self.get_client().await?;
        let room_id_parsed: OwnedRoomId = room_id
            .as_ref()
            .try_into()
            .map_err(|e: IdParseError| AppError::Other(e.to_string()))?;
        let room = client
            .get_room(&room_id_parsed)
            .ok_or_else(|| AppError::Other("Room not found".into()))?;
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

    async fn download_media(&self, event_id: &str, thumbnail: bool) -> Result<Vec<u8>> {
        let client = self.get_client().await?;
        self.media
            .download_media(&client, &self.media_sources, event_id, thumbnail)
            .await
    }

    async fn fetch_user_avatar(&self) -> Result<Option<PathBuf>> {
        let client = self.get_client().await?;
        Ok(self.media.fetch_user_avatar(&client).await)
    }

    async fn subscribe_session_changes(
        &self,
        session_tx: mpsc::UnboundedSender<Session>,
    ) -> Result<()> {
        let client = self.get_client().await?;
        auth::subscribe_session_changes(&client, session_tx).await
    }
}
