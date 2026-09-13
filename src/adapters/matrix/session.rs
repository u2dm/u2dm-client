use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use matrix_sdk::Client;
use matrix_sdk::ruma::{IdParseError, OwnedRoomId};
use tokio::sync::{RwLock, mpsc};

use super::auth;
use super::media::{MatrixMedia, MediaService, MediaSources};
use super::rooms::{MatrixSpaceOrder, MatrixSync};
use super::stickers::MatrixStickers;
use super::store::StoreLayout;
use super::timeline::MatrixTimeline;
use super::verification::MatrixVerification;
use crate::domain::account::AccountScope;
use crate::domain::auth::Session;
use crate::domain::room::RoomId;
use crate::error::{AppError, Result};
use crate::ports::matrix::{AuthenticatedSession, CleanupReport, SessionPort};

pub(super) struct ClientHandle {
    client: RwLock<Option<Client>>,
    layout: StoreLayout,
    account: AccountScope,
    media: Arc<MediaService>,
}

impl ClientHandle {
    pub(super) async fn client(&self) -> Result<Client> {
        self.client
            .read()
            .await
            .clone()
            .ok_or_else(|| AppError::Other("The session has been closed".into()))
    }

    pub(super) fn media(&self) -> &Arc<MediaService> {
        &self.media
    }

    pub(super) async fn room(&self, room_id: &RoomId) -> Result<matrix_sdk::Room> {
        let room_id_parsed: OwnedRoomId = room_id
            .as_ref()
            .try_into()
            .map_err(|e: IdParseError| AppError::Other(e.to_string()))?;
        self.client()
            .await?
            .get_room(&room_id_parsed)
            .ok_or_else(|| AppError::Other("Room not found".into()))
    }

    async fn erase_account(&self) -> CleanupReport {
        let mut report = self.media.close(&self.account).await;
        drop(self.client.write().await.take());
        report.merge(self.layout.purge_account(&self.account).await);
        report
    }
}

#[async_trait]
pub(super) trait SessionResource: Send + Sync {
    async fn release(&self);
}

#[async_trait]
impl SessionResource for MediaSources {
    async fn release(&self) {
        if let Ok(mut sources) = self.lock() {
            sources.clear();
        }
    }
}

struct MatrixLifecycle {
    matrix: Arc<ClientHandle>,
    resources: Vec<Arc<dyn SessionResource>>,
}

impl MatrixLifecycle {
    async fn release_session_resources(&self) {
        for resource in &self.resources {
            resource.release().await;
        }
    }
}

#[async_trait]
impl SessionPort for MatrixLifecycle {
    async fn subscribe_session_changes(
        &self,
        session_tx: mpsc::UnboundedSender<Session>,
    ) -> Result<()> {
        auth::subscribe_session_changes(&self.matrix.client().await?, session_tx).await
    }

    async fn fetch_user_avatar(&self) -> Result<Option<PathBuf>> {
        let client = self.matrix.client().await?;
        Ok(self.matrix.media().fetch_user_avatar(&client).await)
    }

    async fn logout(&self) -> Result<()> {
        tracing::info!("logging out");
        self.release_session_resources().await;
        if let Err(e) = self.matrix.client().await?.logout().await {
            tracing::warn!("failed to logout from server: {e}");
        }
        Ok(())
    }

    async fn clear_store(&self) -> CleanupReport {
        tracing::info!("clearing local account data");
        self.release_session_resources().await;

        let report = self.matrix.erase_account().await;
        if report.is_clean() {
            tracing::info!("local account data cleared");
        } else {
            tracing::warn!("local account data not fully cleared: {}", report.summary());
        }
        report
    }
}

pub(super) async fn authenticate(
    layout: StoreLayout,
    media: Arc<MediaService>,
    client: Client,
    session: Session,
    account: AccountScope,
) -> AuthenticatedSession {
    media.open(&account).await;
    let matrix = Arc::new(ClientHandle {
        client: RwLock::new(Some(client)),
        layout,
        account,
        media,
    });

    let media_sources: Arc<MediaSources> = Arc::new(StdMutex::new(HashMap::new()));
    let verification = Arc::new(MatrixVerification::new(Arc::clone(&matrix)));
    let resources: Vec<Arc<dyn SessionResource>> = vec![
        Arc::clone(&media_sources) as Arc<dyn SessionResource>,
        Arc::clone(&verification) as Arc<dyn SessionResource>,
    ];

    AuthenticatedSession {
        session,
        sync: Arc::new(MatrixSync::new(Arc::clone(&matrix))),
        timeline: Arc::new(MatrixTimeline::new(
            Arc::clone(&matrix),
            Arc::clone(&media_sources),
        )),
        media: Arc::new(MatrixMedia::new(Arc::clone(&matrix), media_sources)),
        verification,
        space_order: Arc::new(MatrixSpaceOrder::new(Arc::clone(&matrix))),
        stickers: Arc::new(MatrixStickers::new(Arc::clone(&matrix))),
        lifecycle: Arc::new(MatrixLifecycle { matrix, resources }),
    }
}
