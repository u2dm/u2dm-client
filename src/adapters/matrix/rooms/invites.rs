use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use matrix_sdk::config::RequestConfig;
use matrix_sdk::ruma::api::client::membership::leave_room;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::{Client, Room, RoomState};
use matrix_sdk_base::{RoomDisplayName, RoomInfoNotableUpdateReasons};
use tokio::time::timeout;

use super::leave::FORGET_WAIT;
use crate::adapters::matrix::member_name::member_name;
use crate::adapters::matrix::session::ClientHandle;
use crate::domain::invite::{Invite, InviteKind};
use crate::domain::member_name::NamedUser;
use crate::domain::room::RoomId;
use crate::error::{AppError, Result};
use crate::ports::matrix::InvitePort;

const SEED_INFLIGHT: usize = 16;

pub(in crate::adapters::matrix) struct MatrixInvites {
    matrix: Arc<ClientHandle>,
}

impl MatrixInvites {
    pub(in crate::adapters::matrix) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self { matrix }
    }
}

#[async_trait]
impl InvitePort for MatrixInvites {
    async fn accept(&self, room_id: &RoomId) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        match room.join().await {
            Ok(()) => Ok(()),
            Err(e) if room.state() == RoomState::Joined => {
                tracing::warn!(%room_id, "joined the room, but finishing the join failed: {e}");
                Ok(())
            }
            Err(e) => Err(AppError::other(e)),
        }
    }

    async fn decline(&self, room_id: &RoomId) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        send_leave(&room).await?;
        mark_left(&room).await?;
        forget(&room).await;
        Ok(())
    }
}

async fn send_leave(room: &Room) -> Result<()> {
    let request = leave_room::v3::Request::new(room.room_id().to_owned());
    match room
        .client()
        .send(request)
        .with_request_config(RequestConfig::short_retry())
        .await
    {
        Ok(_) => Ok(()),
        Err(e) if e.client_api_error_kind() == Some(&ErrorKind::Forbidden) => {
            tracing::debug!(
                room_id = %room.room_id(),
                "the homeserver refused the leave, so the invite is gone already: {e}"
            );
            Ok(())
        }
        Err(e) => Err(AppError::other(e)),
    }
}

async fn mark_left(room: &Room) -> Result<()> {
    room.update_and_save_room_info(|mut info| {
        info.mark_as_left();
        info.mark_state_partially_synced();
        info.mark_members_missing();
        (info, RoomInfoNotableUpdateReasons::MEMBERSHIP)
    })
    .await
    .map_err(AppError::other)
}

async fn forget(room: &Room) {
    let room_id = room.room_id();
    match timeout(FORGET_WAIT, room.forget()).await {
        Ok(Ok(())) => tracing::debug!(%room_id, "forgot the declined room"),
        Ok(Err(e)) => {
            tracing::debug!(%room_id, "declined the invite, but could not forget the room: {e}");
        }
        Err(_) => {
            tracing::debug!(%room_id, "declined the invite; forgetting the room took too long");
        }
    }
}

pub(super) async fn build_invites(client: &Client) -> HashMap<String, Arc<Invite>> {
    stream::iter(client.invited_rooms())
        .map(|room| async move { build_invite(&room).await })
        .buffer_unordered(SEED_INFLIGHT)
        .map(|invite| (invite.id.to_string(), Arc::new(invite)))
        .collect()
        .await
}

pub(super) async fn build_invite(room: &Room) -> Invite {
    let inviter = invited_by(room).await;
    let is_direct = room.is_direct().await.unwrap_or_default();
    let kind = InviteKind::of(room.is_space(), is_direct);
    let name = match room.display_name().await {
        Ok(RoomDisplayName::Empty) | Err(_) => fallback_name(room, inviter.as_ref()),
        Ok(name) => name.to_string(),
    };
    let avatar_mxc =
        room.avatar_url()
            .map(|mxc| mxc.to_string())
            .or_else(|| match (kind, &inviter) {
                (InviteKind::Direct, Some(inviter)) => inviter.avatar_mxc.clone(),
                _ => None,
            });
    Invite {
        id: RoomId::new(room.room_id().to_string()),
        name,
        avatar_mxc,
        topic: room.topic().filter(|topic| !topic.trim().is_empty()),
        kind,
        inviter: inviter.map(|inviter| inviter.user),
        received_ts: room.latest_event_timestamp().map_or(0, |ts| ts.0.into()),
    }
}

struct Inviter {
    user: NamedUser,
    avatar_mxc: Option<String>,
}

async fn invited_by(room: &Room) -> Option<Inviter> {
    let details = match room.invite_details().await {
        Ok(details) => details,
        Err(e) => {
            tracing::debug!(room_id = %room.room_id(), "the invite names no inviter: {e}");
            return None;
        }
    };
    let Some(member) = details.inviter else {
        return Some(Inviter {
            user: NamedUser::unnamed(details.inviter_id.as_str()),
            avatar_mxc: None,
        });
    };
    Some(Inviter {
        user: NamedUser::new(
            details.inviter_id.as_str(),
            member_name(room, &member).await,
        ),
        avatar_mxc: member.avatar_url().map(ToString::to_string),
    })
}

fn fallback_name(room: &Room, inviter: Option<&Inviter>) -> String {
    let Some(inviter) = inviter else {
        return room.room_id().to_string();
    };
    inviter
        .user
        .name
        .set()
        .unwrap_or(&inviter.user.user_id)
        .to_owned()
}
