use std::sync::Arc;

use async_trait::async_trait;
use matrix_sdk::config::RequestConfig;
use matrix_sdk::ruma::api::client::room::create_room;
use matrix_sdk::ruma::api::client::state::send_state_event;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::events::room::encryption::RoomEncryptionEventContent;
use matrix_sdk::ruma::events::room::join_rules::{AllowRule, RoomJoinRulesEventContent};
use matrix_sdk::ruma::events::space::child::SpaceChildEventContent;
use matrix_sdk::ruma::events::space::parent::SpaceParentEventContent;
use matrix_sdk::ruma::events::{AnyInitialStateEvent, InitialStateEvent};
use matrix_sdk::ruma::room::RoomType;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{Int, OwnedRoomId, OwnedServerName};
use matrix_sdk::{Client, HttpError};

use crate::adapters::matrix::permissions::space_powers;
use crate::adapters::matrix::session::ClientHandle;
use crate::adapters::matrix::user_info::own_user;
use crate::domain::room::RoomId;
use crate::domain::room_creation::{
    ChildLink, CreateFailure, CreateResult, CreatedRoom, JoinAccess, RoomBlueprint, Shape,
};
use crate::domain::room_info::ADMIN_LEVEL;
use crate::ports::matrix::RoomCreationPort;

pub(in crate::adapters::matrix) struct MatrixRoomCreation {
    matrix: Arc<ClientHandle>,
}

impl MatrixRoomCreation {
    pub(in crate::adapters::matrix) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self { matrix }
    }
}

#[async_trait]
impl RoomCreationPort for MatrixRoomCreation {
    async fn create_room(&self, blueprint: &RoomBlueprint) -> CreateResult {
        let client = self
            .matrix
            .client()
            .await
            .map_err(|_| CreateFailure::Failed)?;
        let server = own_user(&client)
            .map_err(|_| CreateFailure::Failed)?
            .server_name()
            .to_owned();
        let parent = blueprint.parent().and_then(parsed_room);
        let request = creation_request(blueprint, &server, parent.as_ref()).map_err(|e| {
            tracing::warn!("could not encode the room creation request: {e}");
            CreateFailure::Failed
        })?;
        tracing::info!(kind = ?blueprint.kind(), "creating a room");
        let room_id = client
            .send(request)
            .with_request_config(RequestConfig::short_retry().disable_retry())
            .await
            .map_err(|e| {
                tracing::warn!("the homeserver did not create the room: {e}");
                create_failure(&e)
            })?
            .room_id;
        tracing::info!(%room_id, "created a room");
        let child = match parent {
            Some(parent) => add_child(&client, parent, &room_id, server).await,
            None => ChildLink::NotAsked,
        };
        Ok(CreatedRoom {
            room_id: RoomId::new(room_id.as_str()),
            child,
        })
    }
}

fn parsed_room(room_id: &RoomId) -> Option<OwnedRoomId> {
    OwnedRoomId::try_from(room_id.as_ref())
        .inspect_err(|e| tracing::warn!(%room_id, "not a room id: {e}"))
        .ok()
}

fn creation_request(
    blueprint: &RoomBlueprint,
    server: &OwnedServerName,
    parent: Option<&OwnedRoomId>,
) -> serde_json::Result<create_room::v3::Request> {
    let mut request = create_room::v3::Request::new();
    request.name = Some(blueprint.name().to_owned());
    request.topic = blueprint.topic().map(str::to_owned);
    request.preset = Some(match blueprint.access() {
        JoinAccess::Public(_) => create_room::v3::RoomPreset::PublicChat,
        JoinAccess::InviteOnly | JoinAccess::SpaceMembers(_) => {
            create_room::v3::RoomPreset::PrivateChat
        }
    });
    if let JoinAccess::Public(localpart) = blueprint.access() {
        request.room_alias_name = Some(localpart.as_str().to_owned());
    }
    request.initial_state = initial_state(blueprint, server, parent);
    if blueprint.shape() == Shape::Space {
        let mut creation = create_room::v3::CreationContent::new();
        creation.room_type = Some(RoomType::Space);
        request.creation_content = Some(Raw::new(&creation)?);
        let mut levels = create_room::RoomPowerLevelsContentOverride::new();
        levels.events_default = Int::new(ADMIN_LEVEL);
        request.power_level_content_override = Some(Raw::new(&levels)?);
    }
    Ok(request)
}

fn initial_state(
    blueprint: &RoomBlueprint,
    server: &OwnedServerName,
    parent: Option<&OwnedRoomId>,
) -> Vec<Raw<AnyInitialStateEvent>> {
    let mut state = Vec::new();
    if matches!(blueprint.shape(), Shape::Room { encrypted: true }) {
        state.push(
            InitialStateEvent::with_empty_state_key(
                RoomEncryptionEventContent::with_recommended_defaults(),
            )
            .to_raw_any(),
        );
    }
    if let JoinAccess::SpaceMembers(space) = blueprint.access()
        && let Some(space) = parsed_room(space)
    {
        state.push(
            InitialStateEvent::with_empty_state_key(RoomJoinRulesEventContent::restricted(vec![
                AllowRule::room_membership(space),
            ]))
            .to_raw_any(),
        );
    }
    if let Some(parent) = parent {
        let mut link = SpaceParentEventContent::new(vec![server.clone()]);
        link.canonical = true;
        state.push(InitialStateEvent::new(parent.clone(), link).to_raw_any());
    }
    state
}

async fn add_child(
    client: &Client,
    parent: OwnedRoomId,
    child: &OwnedRoomId,
    server: OwnedServerName,
) -> ChildLink {
    let added = if may_add_children(client, &parent).await {
        send_child(client, parent.clone(), child, server).await
    } else {
        Err("the space's power levels do not let you add rooms".to_owned())
    };
    match added {
        Ok(()) => {
            tracing::info!(%parent, %child, "added the new room to its space");
            ChildLink::Added
        }
        Err(e) => {
            tracing::warn!(%parent, %child, "could not add the new room to its space: {e}");
            ChildLink::Failed
        }
    }
}

async fn may_add_children(client: &Client, space: &OwnedRoomId) -> bool {
    match client.get_room(space) {
        Some(space) => space_powers(&space).await.add_children,
        None => false,
    }
}

async fn send_child(
    client: &Client,
    parent: OwnedRoomId,
    child: &OwnedRoomId,
    server: OwnedServerName,
) -> Result<(), String> {
    let request = send_state_event::v3::Request::new(
        parent,
        child,
        &SpaceChildEventContent::new(vec![server]),
    )
    .map_err(|e| e.to_string())?;
    client
        .send(request)
        .with_request_config(RequestConfig::short_retry())
        .await
        .map(drop)
        .map_err(|e| e.to_string())
}

fn create_failure(error: &HttpError) -> CreateFailure {
    match error.client_api_error_kind() {
        Some(ErrorKind::RoomInUse) => CreateFailure::AddressTaken,
        Some(ErrorKind::Forbidden) => CreateFailure::Refused,
        Some(ErrorKind::UnsupportedRoomVersion | ErrorKind::InvalidRoomState) => {
            CreateFailure::Unsupported
        }
        Some(ErrorKind::LimitExceeded(_)) => CreateFailure::RateLimited,
        _ => CreateFailure::Failed,
    }
}
