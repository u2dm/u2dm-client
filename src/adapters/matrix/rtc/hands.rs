use std::time::Duration;

use matrix_sdk::Room;
use matrix_sdk::config::RequestConfig;
use matrix_sdk::room::{IncludeRelations, RelationsOptions};
use matrix_sdk::ruma::events::AnySyncTimelineEvent;
use matrix_sdk::ruma::events::reaction::ReactionEventContent;
use matrix_sdk::ruma::events::relation::{Annotation, RelationType};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{MilliSecondsSinceUnixEpoch, OwnedEventId, OwnedUserId, UInt};
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::time::timeout;

use crate::adapters::matrix::requests::in_time;
use crate::domain::call::hands::{HandSighting, RAISED_HAND_KEY};
use crate::error::{AppError, Result};

pub(super) const REACTION_TYPE: &str = "m.reaction";
pub(super) const REDACTION_TYPE: &str = "m.room.redaction";
const ANNOTATION: &str = "m.annotation";
const HAND_WAIT: Duration = Duration::from_secs(10);
const HANDS_PER_MEMBERSHIP: u32 = 20;

#[derive(Deserialize)]
struct Reacted {
    event_id: OwnedEventId,
    sender: OwnedUserId,
    origin_server_ts: MilliSecondsSinceUnixEpoch,
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    content: Map<String, Value>,
}

pub(super) fn raises_a_hand(relation: Option<&Value>) -> bool {
    relation
        .and_then(|relation| relation.get("key"))
        .and_then(Value::as_str)
        == Some(RAISED_HAND_KEY)
}

pub(super) fn hand_of(
    reaction: &OwnedEventId,
    sender: &OwnedUserId,
    sent: MilliSecondsSinceUnixEpoch,
    content: &Map<String, Value>,
) -> Option<HandSighting> {
    let relation = content.get("m.relates_to")?.as_object()?;
    if relation.get("rel_type").and_then(Value::as_str) != Some(ANNOTATION)
        || relation.get("key").and_then(Value::as_str) != Some(RAISED_HAND_KEY)
    {
        return None;
    }
    let membership_event = relation.get("event_id").and_then(Value::as_str)?;
    Some(HandSighting {
        by: sender.to_string(),
        membership_event: membership_event.to_owned(),
        reaction: reaction.to_string(),
        raised_ms: sent.0.into(),
    })
}

pub(super) fn redacted_event(
    redacts: Option<&OwnedEventId>,
    content: &Map<String, Value>,
) -> Option<String> {
    redacts.map(ToString::to_string).or_else(|| {
        content
            .get("redacts")
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
}

fn event_id(id: &str) -> Result<OwnedEventId> {
    OwnedEventId::try_from(id).map_err(AppError::other)
}

pub(super) async fn raise(room: &Room, membership_event: &str) -> Result<String> {
    let content = ReactionEventContent::new(Annotation::new(
        event_id(membership_event)?,
        RAISED_HAND_KEY.to_owned(),
    ));
    let sent = in_time(
        HAND_WAIT,
        "the raised hand was not sent in time",
        room.send(content)
            .with_request_config(RequestConfig::short_retry()),
    )
    .await?;
    Ok(sent.response.event_id.to_string())
}

pub(super) async fn lower(room: &Room, reaction: &str) -> Result<()> {
    let reaction = event_id(reaction)?;
    in_time(
        HAND_WAIT,
        "the lowered hand was not sent in time",
        room.redact(&reaction, None, None),
    )
    .await?;
    Ok(())
}

pub(super) async fn raised(room: &Room, memberships: &[String]) -> Vec<HandSighting> {
    let mut found = Vec::new();
    for membership in memberships {
        let Ok(target) = OwnedEventId::try_from(membership.as_str()) else {
            continue;
        };
        let options = RelationsOptions {
            include_relations: IncludeRelations::RelationsOfType(RelationType::Annotation),
            limit: Some(UInt::from(HANDS_PER_MEMBERSHIP)),
            ..RelationsOptions::default()
        };
        match timeout(HAND_WAIT, room.relations(target, options)).await {
            Ok(Ok(relations)) => {
                let mut senders = Vec::new();
                for sighting in relations
                    .chunk
                    .iter()
                    .filter_map(|event| sighting_in(event.raw()))
                {
                    if !senders.contains(&sighting.by) {
                        senders.push(sighting.by.clone());
                        found.push(sighting);
                    }
                }
            }
            Ok(Err(e)) => {
                tracing::debug!(%membership, "could not read the hands raised on a call membership: {e}");
            }
            Err(_) => {
                tracing::debug!(%membership, "the hands raised on a call membership did not arrive in time");
            }
        }
    }
    found
}

fn sighting_in(raw: &Raw<AnySyncTimelineEvent>) -> Option<HandSighting> {
    let event = raw.deserialize_as_unchecked::<Reacted>().ok()?;
    if event.event_type != REACTION_TYPE {
        return None;
    }
    hand_of(
        &event.event_id,
        &event.sender,
        event.origin_server_ts,
        &event.content,
    )
}
