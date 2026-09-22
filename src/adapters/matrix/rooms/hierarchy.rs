use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use matrix_sdk::Client;
use matrix_sdk::ruma::api::client::space::{SpaceHierarchyRoomsChunk, get_hierarchy};
use matrix_sdk::ruma::room::{JoinRuleSummary, RestrictedSummary, RoomType};
use matrix_sdk::ruma::{
    IdParseError, OwnedMxcUri, OwnedRoomId, OwnedServerName, RoomOrAliasId, UInt,
};
use tokio::task::JoinSet;

use super::build::space_child_vias;
use crate::adapters::matrix::media::mxc_avatar_key;
use crate::adapters::matrix::session::ClientHandle;
use crate::domain::room::RoomId;
use crate::domain::space_index::{ChildKind, HierarchyPage, JoinRule, SpaceChild};
use crate::error::{AppError, Result};
use crate::ports::matrix::SpaceIndexPort;

const DIRECT_CHILDREN_ONLY: u32 = 1;
const MAX_INFLIGHT_AVATARS: usize = 8;

type Vias = HashMap<String, Vec<String>>;

pub(in crate::adapters::matrix) struct MatrixSpaceIndex {
    matrix: Arc<ClientHandle>,
}

impl MatrixSpaceIndex {
    pub(in crate::adapters::matrix) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self { matrix }
    }
}

#[async_trait]
impl SpaceIndexPort for MatrixSpaceIndex {
    async fn hierarchy_page(&self, space_id: &RoomId, from: Option<&str>) -> Result<HierarchyPage> {
        let client = self.matrix.client().await?;
        let space = self.matrix.room(space_id).await?;
        let mut request = get_hierarchy::v1::Request::new(space.room_id().to_owned());
        request.max_depth = Some(UInt::from(DIRECT_CHILDREN_ONLY));
        request.from = from.map(ToOwned::to_owned);
        let response = client
            .send(request)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;

        let mut vias: Vias = space_child_vias(&space).await.into_iter().collect();
        let (own, children): (Vec<_>, Vec<_>) = response
            .rooms
            .into_iter()
            .partition(|chunk| chunk.summary.room_id == space.room_id());
        for (id, via) in own.iter().flat_map(announced_vias) {
            vias.entry(id).or_insert(via);
        }

        Ok(HierarchyPage {
            children: children
                .iter()
                .map(|chunk| space_child(&client, chunk, &vias))
                .collect(),
            next: response.next_batch,
        })
    }

    async fn join(&self, room_id: &RoomId, via: &[String]) -> Result<()> {
        let client = self.matrix.client().await?;
        let id: OwnedRoomId = room_id
            .as_ref()
            .try_into()
            .map_err(|e: IdParseError| AppError::Other(e.to_string()))?;
        let servers: Vec<OwnedServerName> = via
            .iter()
            .filter_map(|server| server.as_str().try_into().ok())
            .collect();
        client
            .join_room_by_id_or_alias(<&RoomOrAliasId>::from(&*id), &servers)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        Ok(())
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        let Ok(client) = self.matrix.client().await else {
            return 0;
        };

        let mut fetched = 0;
        let mut tasks: JoinSet<bool> = JoinSet::new();
        for mxc in mxcs {
            let key = mxc_avatar_key(mxc);
            let uri: OwnedMxcUri = mxc.as_str().into();
            let client = client.clone();
            let media = Arc::clone(self.matrix.media());
            tasks.spawn(async move {
                media
                    .fetch_avatar_by_mxc(&client, &key, uri)
                    .await
                    .is_some()
            });
            if tasks.len() >= MAX_INFLIGHT_AVATARS {
                fetched += usize::from(matches!(tasks.join_next().await, Some(Ok(true))));
            }
        }
        while let Some(result) = tasks.join_next().await {
            fetched += usize::from(matches!(result, Ok(true)));
        }
        fetched
    }
}

fn announced_vias(chunk: &SpaceHierarchyRoomsChunk) -> Vec<(String, Vec<String>)> {
    chunk
        .children_state
        .iter()
        .filter_map(|raw| raw.deserialize().ok())
        .map(|child| {
            let via = child.content.via.iter().map(ToString::to_string).collect();
            (child.state_key.to_string(), via)
        })
        .collect()
}

fn space_child(client: &Client, chunk: &SpaceHierarchyRoomsChunk, vias: &Vias) -> SpaceChild {
    let summary = &chunk.summary;
    let id = summary.room_id.to_string();
    let alias = summary.canonical_alias.as_ref().map(ToString::to_string);
    let name = summary
        .name
        .clone()
        .filter(|name| !name.is_empty())
        .or_else(|| cached_name(client, &summary.room_id))
        .or_else(|| alias.clone())
        .unwrap_or_else(|| id.clone());
    let kind = if matches!(summary.room_type, Some(RoomType::Space)) {
        ChildKind::Space {
            children: u64::try_from(chunk.children_state.len()).unwrap_or(u64::MAX),
        }
    } else {
        ChildKind::Room
    };
    SpaceChild {
        via: vias.get(&id).cloned().unwrap_or_default(),
        id: RoomId::new(id),
        name,
        alias,
        topic: summary.topic.clone().filter(|topic| !topic.is_empty()),
        avatar_mxc: summary.avatar_url.as_ref().map(ToString::to_string),
        member_count: summary.num_joined_members.into(),
        join_rule: join_rule(&summary.join_rule),
        kind,
    }
}

fn cached_name(client: &Client, room_id: &OwnedRoomId) -> Option<String> {
    client
        .get_room(room_id)?
        .cached_display_name()
        .map(|name| name.to_string())
}

fn join_rule(summary: &JoinRuleSummary) -> JoinRule {
    match summary {
        JoinRuleSummary::Public => JoinRule::Public,
        JoinRuleSummary::Restricted(rule) => JoinRule::Restricted {
            allowed: allowed_rooms(rule),
        },
        JoinRuleSummary::KnockRestricted(rule) => JoinRule::KnockRestricted {
            allowed: allowed_rooms(rule),
        },
        JoinRuleSummary::Knock => JoinRule::Knock,
        JoinRuleSummary::Invite => JoinRule::Invite,
        JoinRuleSummary::Private => JoinRule::Private,
        _ => JoinRule::Unsupported,
    }
}

fn allowed_rooms(rule: &RestrictedSummary) -> Vec<RoomId> {
    rule.allowed_room_ids
        .iter()
        .map(|id| RoomId::new(id.to_string()))
        .collect()
}
