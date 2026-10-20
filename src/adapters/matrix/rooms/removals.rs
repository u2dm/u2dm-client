use std::collections::HashMap;
use std::sync::Arc;

use futures_util::{StreamExt, stream};
use matrix_sdk::deserialized_responses::MemberEvent;
use matrix_sdk::ruma::events::SyncStateEvent;
use matrix_sdk::ruma::events::room::member::{MembershipChange, MembershipState};
use matrix_sdk::{Client, Room, RoomState};
use matrix_sdk_base::{RoomDisplayName, RoomStateFilter};

use super::build::room_avatar_mxc;
use crate::adapters::matrix::member_name::member_name;
use crate::domain::departure::{Removal, RemovalKind, membership_reason};
use crate::domain::member_name::NamedUser;
use crate::domain::room::RoomId;

const SEED_INFLIGHT: usize = 16;

pub(super) async fn build_removals(client: &Client) -> HashMap<String, Arc<Removal>> {
    let departed = client.rooms_filtered(RoomStateFilter::LEFT | RoomStateFilter::BANNED);
    stream::iter(departed)
        .map(|room| async move { build_removal(&room).await })
        .buffer_unordered(SEED_INFLIGHT)
        .filter_map(|removal| async move { removal })
        .map(|removal| (removal.id.to_string(), Arc::new(removal)))
        .collect()
        .await
}

pub(super) async fn build_removal(room: &Room) -> Option<Removal> {
    let kind = match room.state() {
        RoomState::Left => RemovalKind::Kicked,
        RoomState::Banned => RemovalKind::Banned,
        RoomState::Joined | RoomState::Invited | RoomState::Knocked => return None,
    };
    let room_id = room.room_id();
    let own = match room.get_member_no_sync(room.own_user_id()).await {
        Ok(Some(own)) => own,
        Ok(None) => {
            tracing::debug!(%room_id, "the left room keeps no membership of yours");
            return None;
        }
        Err(e) => {
            tracing::debug!(%room_id, "could not read your membership of the left room: {e}");
            return None;
        }
    };
    let event = own.event();
    if !removed_you(event) {
        return None;
    }
    let is_direct = room.is_direct().await.unwrap_or_default();
    Some(Removal {
        id: RoomId::new(room_id.to_string()),
        name: removal_name(room).await,
        avatar_mxc: room_avatar_mxc(room, is_direct).await,
        kind,
        is_space: room.is_space(),
        is_direct,
        by: remover(room, event).await,
        reason: event
            .reason()
            .map(ToOwned::to_owned)
            .and_then(membership_reason),
        removed_ts: event
            .origin_server_ts()
            .or_else(|| room.latest_event_timestamp())
            .map_or(0, |ts| ts.0.into()),
    })
}

fn removed_you(event: &MemberEvent) -> bool {
    if event.sender() == event.user_id() {
        return false;
    }
    if !matches!(
        event.membership(),
        MembershipState::Leave | MembershipState::Ban
    ) {
        return false;
    }
    let MemberEvent::Sync(SyncStateEvent::Original(original)) = event else {
        return true;
    };
    !matches!(
        original.membership_change(),
        MembershipChange::InvitationRevoked | MembershipChange::KnockDenied
    )
}

async fn remover(room: &Room, event: &MemberEvent) -> NamedUser {
    let sender = event.sender();
    match room.get_member_no_sync(sender).await {
        Ok(Some(member)) => NamedUser::new(sender.as_str(), member_name(room, &member).await),
        Ok(None) => NamedUser::unnamed(sender.as_str()),
        Err(e) => {
            tracing::debug!(room_id = %room.room_id(), "could not read who removed you: {e}");
            NamedUser::unnamed(sender.as_str())
        }
    }
}

async fn removal_name(room: &Room) -> String {
    match room.display_name().await {
        Ok(RoomDisplayName::Empty) | Err(_) => room.room_id().to_string(),
        Ok(name) => name.to_string(),
    }
}
