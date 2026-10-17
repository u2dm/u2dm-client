use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use matrix_sdk::ruma::api::client::delayed_events::update_delayed_event::UpdateAction;
use matrix_sdk::{Client, Room};
use serde_json::{Map, Value, json};

use super::CALL_MEMBER_TYPE;
use super::delayed::{DelayedEvents, restart_failure, schedule_leave};
use super::grants::grant;
use super::transports::own_transports;
use crate::adapters::matrix::session::ClientHandle;
use crate::domain::call::sfu::{GrantFailure, GrantRequest, GrantResult};
use crate::domain::call::{
    CallIntent, DelayedLeave, OwnMembership, RestartFailure, RestartResult, ScheduleFailure,
    ScheduleResult, Secret, SentMembership, Transport,
};
use crate::domain::room::RoomId;
use crate::error::{AppError, Result};
use crate::ports::matrix::RtcMembershipPort;
use crate::util::unix_now_ms;

const OWN_STATE_KEY_VERSIONS: [&str; 2] = ["org.matrix.msc3757", "org.matrix.msc3779"];
const CALL_SUFFIX: &str = "m.call";

pub(in crate::adapters::matrix) struct MatrixRtcMembership {
    matrix: Arc<ClientHandle>,
    delayed: DelayedEvents,
}

impl MatrixRtcMembership {
    pub(in crate::adapters::matrix) const fn new(matrix: Arc<ClientHandle>) -> Self {
        Self {
            matrix,
            delayed: DelayedEvents::new(),
        }
    }

    async fn joined_room(&self, room_id: &RoomId) -> Result<(Client, Room, String)> {
        let client = self.matrix.client().await?;
        let room = self.matrix.room(room_id).await?;
        let key = state_key(&client, &room)?;
        Ok((client, room, key))
    }
}

fn state_key(client: &Client, room: &Room) -> Result<String> {
    let user = client
        .user_id()
        .ok_or_else(|| AppError::Other("the session has no user".into()))?;
    let device = client
        .device_id()
        .ok_or_else(|| AppError::Other("the session has no device".into()))?;
    let key = format!("{user}_{device}_{CALL_SUFFIX}");
    let owns_keys = room
        .version()
        .is_some_and(|version| room_version_owns_keys(version.as_str()));
    Ok(if owns_keys { key } else { format!("_{key}") })
}

fn room_version_owns_keys(version: &str) -> bool {
    OWN_STATE_KEY_VERSIONS.iter().any(|prefix| {
        version.strip_prefix(prefix).is_some_and(|rest| {
            rest.chars()
                .next()
                .is_none_or(|next| !(next.is_alphanumeric() || next == '_'))
        })
    })
}

fn membership_content(room_id: &str, membership: &OwnMembership, now_ms: u64) -> (Value, u64) {
    let created_ms = membership.created_ms.unwrap_or(now_ms);
    let expires_ms = now_ms
        .saturating_add(membership.lifetime_ms)
        .saturating_sub(created_ms);
    let content = json!({
        "application": "m.call",
        "call_id": "",
        "scope": "m.room",
        "device_id": membership.device.as_ref(),
        "membershipID": membership.member_id,
        "expires": expires_ms,
        "m.call.intent": intent_word(membership.intent),
        "focus_active": { "type": "livekit", "focus_selection": "multi_sfu" },
        "foci_preferred": [preferred_focus(room_id, &membership.transport)],
        "created_ts": created_ms,
    });
    (content, created_ms)
}

fn preferred_focus(room_id: &str, transport: &Transport) -> Value {
    let mut focus = Map::new();
    focus.insert("type".into(), Value::from("livekit"));
    if let Some(service) = &transport.service_url {
        focus.insert("livekit_service_url".into(), Value::from(service.as_str()));
    }
    if let Some(sfu) = &transport.sfu_url {
        focus.insert("url".into(), Value::from(sfu.as_str()));
    }
    focus.insert("livekit_alias".into(), Value::from(room_id));
    Value::Object(focus)
}

const fn intent_word(intent: CallIntent) -> &'static str {
    match intent {
        CallIntent::Voice => "audio",
        CallIntent::Video => "video",
    }
}

#[async_trait]
impl RtcMembershipPort for MatrixRtcMembership {
    async fn own_transports(&self) -> Result<Vec<Transport>> {
        let client = self.matrix.client().await?;
        Ok(own_transports(&client).await)
    }

    async fn schedule_leave(&self, room_id: &RoomId, delay: Duration) -> ScheduleResult {
        let (client, room, key) = self
            .joined_room(room_id)
            .await
            .map_err(|e| ScheduleFailure::Failed(e.to_string()))?;
        let delay_id = schedule_leave(&client, room.room_id().to_owned(), key, delay).await?;
        Ok(DelayedLeave {
            room_id: room_id.clone(),
            delay_id: Secret::new(delay_id),
        })
    }

    async fn restart_leave(&self, leave: &DelayedLeave) -> RestartResult {
        let client = self
            .matrix
            .client()
            .await
            .map_err(|e| RestartFailure::Failed(e.to_string()))?;
        self.delayed
            .update(&client, leave.delay_id.expose(), UpdateAction::Restart)
            .await
            .map_err(|e| restart_failure(&e))
    }

    async fn send_leave(&self, leave: &DelayedLeave) -> Result<()> {
        let client = self.matrix.client().await?;
        self.delayed
            .update(&client, leave.delay_id.expose(), UpdateAction::Send)
            .await
            .map_err(|e| AppError::Other(e.to_string()))
    }

    async fn clear_membership(&self, room_id: &RoomId) -> Result<()> {
        let (_, room, key) = self.joined_room(room_id).await?;
        room.send_state_event_raw(CALL_MEMBER_TYPE, &key, json!({}))
            .await
            .map(|_| ())
            .map_err(|e| AppError::Other(e.to_string()))
    }

    async fn send_membership(
        &self,
        room_id: &RoomId,
        membership: &OwnMembership,
    ) -> Result<SentMembership> {
        let (_, room, key) = self.joined_room(room_id).await?;
        let (content, created_ms) = membership_content(room_id.as_ref(), membership, unix_now_ms());
        let response = room
            .send_state_event_raw(CALL_MEMBER_TYPE, &key, content)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        Ok(SentMembership {
            event_id: response.event_id.to_string(),
            created_ms,
        })
    }

    async fn sfu_grant(&self, room_id: &RoomId, request: &GrantRequest) -> GrantResult {
        let client = self
            .matrix
            .client()
            .await
            .map_err(|e| GrantFailure::Unreachable(e.to_string()))?;
        grant(&client, room_id.as_ref(), request).await
    }
}
