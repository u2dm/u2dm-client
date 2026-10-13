use std::sync::Arc;

use async_trait::async_trait;
use matrix_sdk::deserialized_responses::SyncOrStrippedState;
use matrix_sdk::ruma::events::SyncStateEvent;
use matrix_sdk::ruma::events::room::tombstone::RoomTombstoneEventContent;
use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId, RoomId as MatrixRoomId, RoomOrAliasId};
use matrix_sdk::{Client, Room, RoomState};

use crate::adapters::matrix::session::ClientHandle;
use crate::domain::room::RoomId;
use crate::error::{AppError, Result};
use crate::ports::matrix::UpgradePort;

pub(in crate::adapters::matrix) struct MatrixUpgrades {
    matrix: Arc<ClientHandle>,
}

impl MatrixUpgrades {
    pub(in crate::adapters::matrix) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self { matrix }
    }
}

struct LinkedRoom {
    room_id: OwnedRoomId,
    named_by: OwnedUserId,
}

#[async_trait]
impl UpgradePort for MatrixUpgrades {
    async fn join_successor(&self, room_id: &RoomId) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        let successor = successor(&room).await?;
        join(&room.client(), &successor).await
    }

    async fn join_predecessor(&self, room_id: &RoomId) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        let predecessor = predecessor(&room)?;
        join(&room.client(), &predecessor).await
    }
}

async fn successor(room: &Room) -> Result<LinkedRoom> {
    let raw = room
        .get_state_event_static::<RoomTombstoneEventContent>()
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
        .ok_or_else(|| AppError::Other("The room has no tombstone".into()))?;
    let tombstone = raw
        .deserialize()
        .map_err(|e| AppError::Other(e.to_string()))?;
    match tombstone {
        SyncOrStrippedState::Sync(SyncStateEvent::Original(tombstone)) => Ok(LinkedRoom {
            room_id: tombstone.content.replacement_room,
            named_by: tombstone.sender,
        }),
        SyncOrStrippedState::Sync(SyncStateEvent::Redacted(_))
        | SyncOrStrippedState::Stripped(_) => Err(AppError::Other(
            "The room's tombstone names no replacement room".into(),
        )),
    }
}

fn predecessor(room: &Room) -> Result<LinkedRoom> {
    let create = room
        .create_content()
        .ok_or_else(|| AppError::Other("The room's creation event is not known".into()))?;
    let predecessor = create
        .predecessor
        .ok_or_else(|| AppError::Other("The room replaces no other room".into()))?;
    Ok(LinkedRoom {
        room_id: predecessor.room_id,
        named_by: create.creator,
    })
}

async fn join(client: &Client, linked: &LinkedRoom) -> Result<()> {
    let server = linked.named_by.server_name();
    tracing::info!(
        room_id = %linked.room_id,
        via = %server,
        "joining the other end of a room upgrade"
    );
    match client
        .join_room_by_id_or_alias(
            <&RoomOrAliasId>::from(&*linked.room_id),
            &[server.to_owned()],
        )
        .await
    {
        Ok(_) => Ok(()),
        Err(e) if is_joined(client, &linked.room_id) => {
            tracing::warn!(
                room_id = %linked.room_id,
                "joined the room, but finishing the join failed: {e}"
            );
            Ok(())
        }
        Err(e) => Err(AppError::Other(e.to_string())),
    }
}

fn is_joined(client: &Client, room_id: &MatrixRoomId) -> bool {
    client
        .get_room(room_id)
        .is_some_and(|room| room.state() == RoomState::Joined)
}
