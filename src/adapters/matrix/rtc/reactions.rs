use std::time::Duration;

use matrix_sdk::Room;
use matrix_sdk::config::RequestConfig;
use matrix_sdk::ruma::{OwnedEventId, OwnedUserId};
use serde_json::{Map, Value, json};

use super::REFERENCE;
use crate::adapters::matrix::requests::in_time;
use crate::domain::call::reactions::{ReactionOption, ReactionSighting};
use crate::error::Result;

pub(super) const CALL_REACTION_TYPE: &str = "io.element.call.reaction";
const REACTION_WAIT: Duration = Duration::from_secs(10);

pub(super) fn reaction_of(
    event_id: &OwnedEventId,
    sender: &OwnedUserId,
    content: &Map<String, Value>,
    age: Option<Duration>,
) -> Option<ReactionSighting> {
    let relation = content.get("m.relates_to")?.as_object()?;
    if relation.get("rel_type").and_then(Value::as_str) != Some(REFERENCE) {
        return None;
    }
    let membership_event = relation.get("event_id").and_then(Value::as_str)?;
    let emoji = content.get("emoji").and_then(Value::as_str)?;
    let name = content
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(ReactionSighting {
        by: sender.to_string(),
        membership_event: membership_event.to_owned(),
        reaction: event_id.to_string(),
        emoji: emoji.to_owned(),
        name: name.to_owned(),
        age,
    })
}

pub(super) fn reaction_content(membership_event: &str, reaction: ReactionOption) -> Value {
    json!({
        "m.relates_to": { "rel_type": REFERENCE, "event_id": membership_event },
        "emoji": reaction.emoji,
        "name": reaction.name,
    })
}

pub(super) async fn react(
    room: &Room,
    membership_event: &str,
    reaction: ReactionOption,
) -> Result<String> {
    let content = reaction_content(membership_event, reaction);
    let sent = in_time(
        REACTION_WAIT,
        "the call reaction was not sent in time",
        room.send_raw(CALL_REACTION_TYPE, content)
            .with_request_config(RequestConfig::short_retry()),
    )
    .await?;
    Ok(sent.response.event_id.to_string())
}
