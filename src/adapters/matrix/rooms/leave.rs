use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use matrix_sdk::config::RequestConfig;
use matrix_sdk::ruma::api::client::state::get_state_event_for_key;
use matrix_sdk::ruma::events::StateEventType;
use matrix_sdk::ruma::events::room::member::{MembershipState, RoomMemberEventContent};
use matrix_sdk::ruma::events::room::power_levels::RoomPowerLevels;
use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId, UserId};
use matrix_sdk::{Client, Room, RoomState};
use tokio::time::timeout;

use crate::adapters::matrix::permissions::member_role;
use crate::adapters::matrix::requests::{absent_as_none, in_time};
use crate::adapters::matrix::session::ClientHandle;
use crate::domain::departure::AdminStanding;
use crate::domain::room::RoomId;
use crate::domain::room_info::MemberRole;
use crate::error::{AppError, Result};
use crate::ports::matrix::LeavePort;

pub(super) const FORGET_WAIT: Duration = Duration::from_secs(5);
const CAPABILITIES_WAIT: Duration = Duration::from_secs(5);

pub(in crate::adapters::matrix) struct MatrixLeaves {
    matrix: Arc<ClientHandle>,
}

impl MatrixLeaves {
    pub(in crate::adapters::matrix) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self { matrix }
    }
}

#[async_trait]
impl LeavePort for MatrixLeaves {
    async fn leave(&self, room_id: &RoomId) -> Result<()> {
        let client = self.matrix.client().await?;
        let room = self.matrix.room(room_id).await?;
        let versions = versions_left_with(&client, &room);
        room.leave().await.map_err(AppError::other)?;
        if forgets_on_leave(&client).await {
            tracing::info!(%room_id, "left the room; the homeserver forgets it by itself");
            return Ok(());
        }
        for version in &versions {
            forget_left(version).await;
        }
        Ok(())
    }

    async fn forget(&self, room_id: &RoomId) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        in_time(
            FORGET_WAIT,
            "the homeserver took too long to forget the room",
            room.forget(),
        )
        .await?;
        tracing::info!(%room_id, "forgot the room you were removed from");
        Ok(())
    }

    async fn admin_standing(&self, room_id: &RoomId) -> Result<AdminStanding> {
        let room = self.matrix.room(room_id).await?;
        let levels = room.power_levels_or_default().await;
        let own = room.own_user_id();
        if !is_admin(&levels, own) {
            return Ok(AdminStanding::NotOnlyAdmin);
        }
        for admin in other_admins(&levels, own) {
            if membership_of(&room, &admin).await? == Some(MembershipState::Join) {
                return Ok(AdminStanding::NotOnlyAdmin);
            }
        }
        tracing::debug!(%room_id, "no other joined member can manage the room");
        Ok(AdminStanding::OnlyAdmin)
    }
}

fn versions_left_with(client: &Client, room: &Room) -> Vec<Room> {
    let mut versions = vec![room.clone()];
    let mut seen: HashSet<OwnedRoomId> = HashSet::from([room.room_id().to_owned()]);
    let mut current = room.clone();
    while let Some(predecessor) = current.predecessor_room() {
        if !seen.insert(predecessor.room_id.clone()) {
            break;
        }
        let Some(older) = client.get_room(&predecessor.room_id) else {
            break;
        };
        if matches!(
            older.state(),
            RoomState::Joined | RoomState::Invited | RoomState::Knocked
        ) {
            versions.push(older.clone());
        }
        current = older;
    }
    versions
}

async fn forgets_on_leave(client: &Client) -> bool {
    let capabilities = client.homeserver_capabilities();
    match timeout(CAPABILITIES_WAIT, capabilities.forgets_room_when_leaving()).await {
        Ok(Ok(forced)) => forced,
        Ok(Err(e)) => {
            tracing::debug!("could not read whether the homeserver forgets left rooms: {e}");
            false
        }
        Err(_) => {
            tracing::debug!("reading whether the homeserver forgets left rooms took too long");
            false
        }
    }
}

async fn forget_left(room: &Room) {
    let room_id = room.room_id();
    match timeout(FORGET_WAIT, room.forget()).await {
        Ok(Ok(())) => tracing::info!(%room_id, "forgot the room you left"),
        Ok(Err(e)) => tracing::warn!(%room_id, "left the room, but could not forget it: {e}"),
        Err(_) => tracing::warn!(%room_id, "left the room; forgetting it took too long"),
    }
}

fn is_admin(levels: &RoomPowerLevels, user: &UserId) -> bool {
    member_role(levels.for_user(user)) >= MemberRole::Admin
}

fn other_admins(levels: &RoomPowerLevels, own: &UserId) -> Vec<OwnedUserId> {
    let creators = levels.rules.privileged_creators.iter().flatten().cloned();
    let mut admins: Vec<OwnedUserId> = levels
        .users
        .keys()
        .cloned()
        .chain(creators)
        .filter(|user| user != own && is_admin(levels, user))
        .collect();
    admins.sort();
    admins.dedup();
    admins
}

async fn membership_of(room: &Room, user: &UserId) -> Result<Option<MembershipState>> {
    match room.get_member_no_sync(user).await {
        Ok(Some(member)) => return Ok(Some(member.membership().clone())),
        Ok(None) => {}
        Err(e) => tracing::debug!(%user, "could not read a membership from the store: {e}"),
    }
    let request = get_state_event_for_key::v3::Request::new(
        room.room_id().to_owned(),
        StateEventType::RoomMember,
        user.to_string(),
    );
    let sent = room
        .client()
        .send(request)
        .with_request_config(RequestConfig::short_retry())
        .await;
    let Some(response) = absent_as_none(sent)? else {
        return Ok(None);
    };
    response
        .into_content()
        .deserialize_as_unchecked::<RoomMemberEventContent>()
        .map(|content| Some(content.membership))
        .map_err(AppError::other)
}
