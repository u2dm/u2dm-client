use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use matrix_sdk::room_preview::RoomPreview as SdkRoomPreview;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::room::RoomType;
use matrix_sdk::ruma::{
    OwnedRoomId, OwnedRoomOrAliasId, OwnedServerName, RoomId as MatrixRoomId, RoomOrAliasId,
};
use matrix_sdk::{Error as SdkError, RoomState};
use tokio::time::timeout;

use super::upgrades::is_joined;
use crate::adapters::matrix::media::fetch_avatar_thumbnails;
use crate::adapters::matrix::room_state::join_rule;
use crate::adapters::matrix::session::ClientHandle;
use crate::domain::matrix_link::RoomAddress;
use crate::domain::room::RoomId;
use crate::domain::room_preview::{PreviewFailure, PreviewMembership, PreviewResult, RoomPreview};
use crate::error::{AppError, Result};
use crate::ports::matrix::RoomPreviewPort;

const PREVIEW_WAIT: Duration = Duration::from_secs(20);

pub(in crate::adapters::matrix) struct MatrixRoomPreviews {
    matrix: Arc<ClientHandle>,
}

impl MatrixRoomPreviews {
    pub(in crate::adapters::matrix) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self { matrix }
    }
}

#[async_trait]
impl RoomPreviewPort for MatrixRoomPreviews {
    async fn preview(&self, room: &RoomAddress, via: &[String]) -> PreviewResult {
        let client = self
            .matrix
            .client()
            .await
            .map_err(|_| PreviewFailure::Failed)?;
        let address = room_or_alias(room).ok_or(PreviewFailure::NoSuchRoom)?;
        let servers = server_names(via);
        tracing::info!(room = %address, via = ?servers, "previewing a linked room");
        match timeout(PREVIEW_WAIT, client.get_room_preview(&address, servers)).await {
            Ok(Ok(preview)) => Ok(room_preview(preview)),
            Ok(Err(e)) => {
                tracing::warn!(room = %address, "the linked room could not be previewed: {e}");
                Err(failure(&e))
            }
            Err(_) => {
                tracing::warn!(room = %address, "the linked room's preview did not arrive in time");
                Err(PreviewFailure::Failed)
            }
        }
    }

    async fn join(
        &self,
        room: &RoomAddress,
        known: Option<&RoomId>,
        via: &[String],
    ) -> Result<RoomId> {
        let client = self.matrix.client().await?;
        let address = room_or_alias(room)
            .ok_or_else(|| AppError::Other(format!("{room} is not a room address")))?;
        let servers = join_servers(&address, via);
        tracing::info!(room = %address, via = ?servers, "joining a linked room");
        match client.join_room_by_id_or_alias(&address, &servers).await {
            Ok(joined) => Ok(RoomId::new(joined.room_id().as_str())),
            Err(e) => match joined_anyway(&address, known).filter(|id| is_joined(&client, id)) {
                Some(room_id) => {
                    tracing::warn!(%room_id, "joined the room, but finishing the join failed: {e}");
                    Ok(RoomId::new(room_id.as_str()))
                }
                None => Err(AppError::other(e)),
            },
        }
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        let Ok(client) = self.matrix.client().await else {
            return 0;
        };
        fetch_avatar_thumbnails(&client, self.matrix.media(), mxcs).await
    }
}

fn room_preview(preview: SdkRoomPreview) -> RoomPreview {
    let id = preview.room_id.to_string();
    let alias = preview.canonical_alias.as_ref().map(ToString::to_string);
    let name = preview
        .name
        .filter(|name| !name.is_empty())
        .or_else(|| alias.clone())
        .unwrap_or_else(|| id.clone());
    RoomPreview {
        id: RoomId::new(id),
        name,
        alias,
        topic: preview.topic.filter(|topic| !topic.is_empty()),
        avatar_mxc: preview.avatar_url.as_ref().map(ToString::to_string),
        member_count: preview.num_joined_members,
        is_space: matches!(preview.room_type, Some(RoomType::Space)),
        join_rule: preview.join_rule.as_ref().map(join_rule),
        membership: membership(preview.state),
    }
}

fn membership(state: Option<RoomState>) -> PreviewMembership {
    match state {
        Some(RoomState::Joined) => PreviewMembership::Joined,
        Some(RoomState::Invited) => PreviewMembership::Invited,
        Some(RoomState::Knocked) => PreviewMembership::Knocked,
        Some(RoomState::Banned) => PreviewMembership::Banned,
        Some(RoomState::Left) | None => PreviewMembership::Outsider,
    }
}

fn failure(error: &SdkError) -> PreviewFailure {
    if matches!(error, SdkError::InsufficientData) {
        return PreviewFailure::Unavailable;
    }
    if error.client_api_error_kind() == Some(&ErrorKind::NotFound) {
        return PreviewFailure::NoSuchRoom;
    }
    PreviewFailure::Failed
}

fn room_or_alias(room: &RoomAddress) -> Option<OwnedRoomOrAliasId> {
    RoomOrAliasId::parse(room.as_str()).ok()
}

fn server_names(via: &[String]) -> Vec<OwnedServerName> {
    via.iter()
        .filter_map(|server| server.as_str().try_into().ok())
        .collect()
}

fn join_servers(address: &RoomOrAliasId, via: &[String]) -> Vec<OwnedServerName> {
    let servers = server_names(via);
    if !servers.is_empty() || address.is_room_alias_id() {
        return servers;
    }
    address
        .server_name()
        .map(ToOwned::to_owned)
        .into_iter()
        .collect()
}

fn joined_anyway(address: &RoomOrAliasId, known: Option<&RoomId>) -> Option<OwnedRoomId> {
    known
        .and_then(|room_id| MatrixRoomId::parse(room_id.as_ref()).ok())
        .or_else(|| {
            <&MatrixRoomId>::try_from(address)
                .ok()
                .map(ToOwned::to_owned)
        })
}
