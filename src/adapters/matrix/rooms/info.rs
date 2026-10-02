use std::sync::Arc;

use async_trait::async_trait;
use matrix_sdk::deserialized_responses::SyncOrStrippedState;
use matrix_sdk::room::RoomMember;
use matrix_sdk::ruma::events::SyncStateEvent;
use matrix_sdk::ruma::events::room::member::{MembershipChange, MembershipState};
use matrix_sdk::ruma::events::room::power_levels::UserPowerLevel;
use matrix_sdk::ruma::events::room::topic::RoomTopicEventContent;
use matrix_sdk::{Room, RoomMemberships};

use super::build::{default_notification_mode, notification_mode_for};
use crate::adapters::matrix::media::fetch_avatar_thumbnails;
use crate::adapters::matrix::session::ClientHandle;
use crate::domain::message::RichText;
use crate::domain::room::{NotifyMode, RoomId};
use crate::domain::room_info::{MemberRole, RoomAbout, RosterMember, RosterSection, sort_roster};
use crate::error::{AppError, Result};
use crate::ports::matrix::RoomInfoPort;

pub(in crate::adapters::matrix) struct MatrixRoomInfo {
    matrix: Arc<ClientHandle>,
}

impl MatrixRoomInfo {
    pub(in crate::adapters::matrix) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self { matrix }
    }
}

#[async_trait]
impl RoomInfoPort for MatrixRoomInfo {
    async fn about(&self, room_id: &RoomId) -> Result<RoomAbout> {
        let room = self.matrix.room(room_id).await?;
        let joined_at = match room.get_member_no_sync(room.own_user_id()).await {
            Ok(own) => own.as_ref().and_then(joined_at),
            Err(e) => {
                tracing::debug!(%room_id, "could not read the own membership: {e}");
                None
            }
        };
        let link = match room.matrix_to_permalink().await {
            Ok(link) => link.to_string(),
            Err(e) => {
                tracing::debug!(%room_id, "linking the bare room id: {e}");
                room.room_id().matrix_to_uri().to_string()
            }
        };
        Ok(RoomAbout {
            joined_at,
            link,
            topic: rich_topic(&room).await,
        })
    }

    async fn roster(&self, room_id: &RoomId) -> Result<Vec<RosterMember>> {
        let room = self.matrix.room(room_id).await?;
        let members = room
            .members(RoomMemberships::ACTIVE)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        let mut roster: Vec<RosterMember> = members.iter().filter_map(roster_member).collect();
        sort_roster(&mut roster);
        Ok(roster)
    }

    async fn set_notify(&self, room_id: &RoomId, mode: NotifyMode) -> Result<()> {
        let client = self.matrix.client().await?;
        let room = self.matrix.room(room_id).await?;
        let settings = client.notification_settings().await;
        let target = notification_mode_for(mode);
        let written = if default_notification_mode(&room, &settings).await == target {
            settings
                .delete_user_defined_room_rules(room.room_id())
                .await
        } else {
            settings
                .set_room_notification_mode(room.room_id(), target)
                .await
        };
        written.map_err(|e| AppError::Other(e.to_string()))
    }

    async fn leave(&self, room_id: &RoomId) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        room.leave()
            .await
            .map_err(|e| AppError::Other(e.to_string()))
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        let Ok(client) = self.matrix.client().await else {
            return 0;
        };
        fetch_avatar_thumbnails(&client, self.matrix.media(), mxcs).await
    }
}

async fn rich_topic(room: &Room) -> Option<RichText> {
    let raw = match room.get_state_event_static::<RoomTopicEventContent>().await {
        Ok(raw) => raw?,
        Err(e) => {
            tracing::debug!(room = %room.room_id(), "could not read the topic: {e}");
            return None;
        }
    };
    let SyncOrStrippedState::Sync(SyncStateEvent::Original(event)) = raw.deserialize().ok()? else {
        return None;
    };
    let content = event.content;
    if content.topic.trim().is_empty() {
        return None;
    }
    Some(match content.topic_block.text.find_html() {
        Some(html) => RichText::formatted(content.topic, html.to_owned()),
        None => RichText::plain(content.topic),
    })
}

fn joined_at(member: &RoomMember) -> Option<u64> {
    let SyncOrStrippedState::Sync(SyncStateEvent::Original(event)) = member.event().as_ref() else {
        return None;
    };
    matches!(
        event.membership_change(),
        MembershipChange::Joined | MembershipChange::InvitationAccepted
    )
    .then(|| u64::from(event.origin_server_ts.0))
}

fn roster_member(member: &RoomMember) -> Option<RosterMember> {
    let section = match member.membership() {
        MembershipState::Join => RosterSection::Joined,
        MembershipState::Invite => RosterSection::Invited,
        _ => return None,
    };
    Some(RosterMember {
        user_id: member.user_id().to_string(),
        display_name: member.display_name().map(ToOwned::to_owned),
        avatar_mxc: member.avatar_url().map(ToString::to_string),
        role: role(member.power_level()),
        section,
    })
}

fn role(level: UserPowerLevel) -> MemberRole {
    match level {
        UserPowerLevel::Infinite => MemberRole::Owner,
        UserPowerLevel::Int(level) => MemberRole::for_level(level.into()),
        _ => MemberRole::Member,
    }
}
