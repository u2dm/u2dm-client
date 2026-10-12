mod own;
mod room;

use std::sync::Arc;

use async_trait::async_trait;
use matrix_sdk::encryption::identities::UserIdentity;
use tokio::sync::{Notify, mpsc};

use super::session::ClientHandle;
use super::user_info::parse_user;
use crate::domain::identity::{OwnIdentity, RoomAlert};
use crate::domain::room::RoomId;
use crate::domain::user_info::UserId;
use crate::error::{AppError, Result};
use crate::ports::matrix::IdentityPort;

pub(super) struct MatrixIdentityAlerts {
    matrix: Arc<ClientHandle>,
    own_acknowledged: Notify,
}

impl MatrixIdentityAlerts {
    pub(super) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self {
            matrix,
            own_acknowledged: Notify::new(),
        }
    }

    async fn identity_of(&self, user_id: &UserId) -> Result<UserIdentity> {
        let user = parse_user(user_id)?;
        self.matrix
            .client()
            .await?
            .encryption()
            .get_user_identity(&user)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?
            .ok_or_else(|| AppError::Other(format!("no identity is known for {user_id}")))
    }
}

#[async_trait]
impl IdentityPort for MatrixIdentityAlerts {
    async fn follow_room(
        &self,
        room_id: &RoomId,
        alerts_tx: mpsc::Sender<Vec<RoomAlert>>,
    ) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        room::follow(&room, alerts_tx).await
    }

    async fn follow_own(&self, own_tx: mpsc::Sender<OwnIdentity>) -> Result<()> {
        let client = self.matrix.client().await?;
        own::follow(&client, &self.own_acknowledged, own_tx).await
    }

    async fn pin_identity(&self, user_id: &UserId) -> Result<()> {
        self.identity_of(user_id)
            .await?
            .pin()
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        tracing::info!(%user_id, "accepted the user's new identity");
        Ok(())
    }

    async fn withdraw_verification(&self, user_id: &UserId) -> Result<()> {
        self.identity_of(user_id)
            .await?
            .withdraw_verification()
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        tracing::info!(%user_id, "withdrew the verification of the user");
        Ok(())
    }

    async fn send_anyway(&self, room_id: &RoomId, user_id: &UserId) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        room::send_anyway(&room, &parse_user(user_id)?).await
    }

    async fn acknowledge_own_reset(&self) -> Result<()> {
        own::acknowledge(&self.matrix.client().await?).await?;
        self.own_acknowledged.notify_one();
        Ok(())
    }
}
