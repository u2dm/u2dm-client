use matrix_sdk::Room;
use matrix_sdk::ruma::{EventId, OwnedEventId};
use matrix_sdk_ui::timeline::{Timeline, TimelineEventItemId};
use serde_json::{Map, Value};

use crate::adapters::matrix::timeline::convert::reaction_send;
use crate::adapters::matrix::timeline::send_gate::refuse_unpermitted_toggle;
use crate::domain::emote::Emote;
use crate::domain::message::ReactionSend;
use crate::error::{AppError, Result};

const REACTION_SHORTCODE_FIELD: &str = "com.beeper.reaction.shortcode";

async fn own_reaction(timeline: &Timeline, target: &EventId, key: &str) -> Option<ReactionSend> {
    let item = timeline.item_by_event_id(target).await?;
    let own = timeline.room().own_user_id();
    item.content()
        .reactions()
        .and_then(|by_key| by_key.get(key))
        .and_then(|by_sender| by_sender.get(own))
        .map(|info| reaction_send(Some(info)))
}

#[derive(Debug)]
enum ReactionToggle {
    Added,
    Removed,
    DiscardedFailedSend,
}

async fn toggle_own_reaction(
    timeline: &Timeline,
    target: OwnedEventId,
    key: &str,
    shortcode: Option<&str>,
) -> Result<ReactionToggle> {
    let own = own_reaction(timeline, &target, key).await;
    refuse_unpermitted_toggle(timeline.room(), own).await?;
    let added = timeline
        .toggle_reaction_with_extra_content(
            &TimelineEventItemId::EventId(target),
            key,
            shortcode.map(reaction_shortcode),
        )
        .await
        .map_err(AppError::other)?;
    Ok(match (added, own) {
        (true, _) => ReactionToggle::Added,
        (false, Some(ReactionSend::Failed)) => ReactionToggle::DiscardedFailedSend,
        (false, _) => ReactionToggle::Removed,
    })
}

fn resume_after_discarded_reaction(room: &Room, event_id: &str) {
    room.send_queue().set_enabled(true);
    tracing::info!(
        event_id,
        "discarded a failed reaction, resuming the room's send queue"
    );
}

fn reaction_shortcode(shortcode: &str) -> Map<String, Value> {
    Map::from_iter([(
        REACTION_SHORTCODE_FIELD.to_owned(),
        Value::String(Emote::token_for_title(shortcode)),
    )])
}

pub(super) async fn toggle_reaction(
    timeline: &Timeline,
    event_id: &str,
    key: &str,
    shortcode: Option<&str>,
) {
    let Ok(target) = OwnedEventId::try_from(event_id) else {
        tracing::warn!(
            event_id,
            "ignoring a reaction toggle for a malformed event id"
        );
        return;
    };
    match toggle_own_reaction(timeline, target, key, shortcode).await {
        Ok(ReactionToggle::DiscardedFailedSend) => {
            resume_after_discarded_reaction(timeline.room(), event_id);
        }
        Ok(toggle) => tracing::debug!(event_id, ?toggle, "toggled a reaction"),
        Err(e) => tracing::warn!("failed to toggle a reaction: {e}"),
    }
}
