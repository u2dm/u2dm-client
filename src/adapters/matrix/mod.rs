mod auth;
mod media;
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
use matrix_sdk::ruma::IdParseError;
use matrix_sdk::ruma::OwnedRoomId;
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
use matrix_sdk::utils::local_server::LocalServerRedirectHandle;
use matrix_sdk_ui::timeline::Timeline;
use tokio::sync::{Mutex, RwLock, mpsc};

use crate::domain::models::{
    LoginCredentials, OAuthLoginData, Room as DomainRoom, RoomId, ServerInfo, Session, SyncEvent,
    TimelinePatch, VerificationEvent,
};
use crate::error::{AppError, Result};
use crate::ports::matrix::MatrixPort;

pub struct MatrixAdapter {
    data_dir: PathBuf,
    client: RwLock<Option<Client>>,
    redirect_handle: Mutex<Option<LocalServerRedirectHandle>>,
    verification_request: Mutex<Option<VerificationRequest>>,
    sas_verification: Mutex<Option<SasVerification>>,
    media_sources: Arc<StdMutex<HashMap<String, MediaSource>>>,
    active_timeline: Mutex<Option<Timeline>>,
    verification_req_rx: Mutex<Option<mpsc::UnboundedReceiver<VerificationRequest>>>,
    verification_handler_guards: Mutex<Vec<EventHandlerDropGuard>>,
}

impl MatrixAdapter {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            client: RwLock::new(None),
            redirect_handle: Mutex::new(None),
            verification_request: Mutex::new(None),
            sas_verification: Mutex::new(None),
            media_sources: Arc::new(StdMutex::new(HashMap::new())),
            active_timeline: Mutex::new(None),
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
}

#[async_trait]
impl MatrixPort for MatrixAdapter {
    async fn discover_auth(&self, homeserver: &str, passphrase: &str) -> Result<ServerInfo> {
        auth::discover_auth(&self.client, &self.data_dir, homeserver, passphrase).await
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

    async fn rooms(&self) -> Result<Vec<DomainRoom>> {
        let client = self.get_client().await?;
        rooms::fetch_rooms(&client).await
    }

    async fn subscribe_timeline(
        &self,
        room_id: &RoomId,
        timeline_tx: mpsc::UnboundedSender<TimelinePatch>,
    ) -> Result<()> {
        let client = self.get_client().await?;
        let result = timeline::subscribe_timeline(
            &client,
            &self.data_dir,
            &self.media_sources,
            room_id,
            timeline_tx,
            &self.active_timeline,
        )
        .await;
        *self.active_timeline.lock().await = None;
        result
    }

    async fn start_sync(&self, on_sync: Box<dyn Fn(SyncEvent) + Send + Sync>) -> Result<()> {
        let client = self.get_client().await?;
        rooms::start_sync(&client, on_sync.into()).await
    }

    async fn restore_session(&self, session: &Session, passphrase: &str) -> Result<()> {
        auth::restore_session(&self.client, &self.data_dir, session, passphrase).await
    }

    async fn logout(&self) -> Result<()> {
        auth::logout(
            &self.client,
            &self.verification_req_rx,
            &self.verification_handler_guards,
        )
        .await
    }

    async fn clear_store(&self) -> Result<()> {
        auth::clear_store(
            &self.client,
            &self.data_dir,
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
            .0
            .as_str()
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

    async fn download_media(&self, event_id: &str, thumbnail: bool) -> Result<Vec<u8>> {
        let client = self.get_client().await?;
        media::download_media(&client, &self.media_sources, event_id, thumbnail).await
    }

    async fn subscribe_session_changes(
        &self,
        session_tx: mpsc::UnboundedSender<Session>,
    ) -> Result<()> {
        let client = self.get_client().await?;
        auth::subscribe_session_changes(&client, session_tx).await
    }
}
