use std::slice;
use std::sync::Arc;

use matrix_sdk_ui::timeline::TimelineItem;

use super::TimelineContext;
use super::convert::convert_event_item;
use crate::adapters::matrix::media::try_enrich_from_cache;
use crate::domain::models::TimelineMessage;

pub(super) fn is_renderable(item: &TimelineItem) -> bool {
    let Some(event) = item.as_event() else {
        return false;
    };
    let content = event.content();
    content.as_message().is_some() || content.as_unable_to_decrypt().is_some()
}

pub(super) fn msg_index_at(items: &[Arc<TimelineItem>], raw_index: usize) -> usize {
    items
        .get(..raw_index)
        .unwrap_or(items)
        .iter()
        .filter(|i| is_renderable(i))
        .count()
}

pub(super) fn convert_timeline_items(
    items: &[Arc<TimelineItem>],
    ctx: &TimelineContext<'_>,
) -> Vec<TimelineMessage> {
    items
        .iter()
        .filter_map(|item| convert_event_item(item.as_event()?, ctx.media_sources, ctx.own_user_id))
        .collect()
}

pub(super) fn convert_and_enrich_from_cache(
    item: &Arc<TimelineItem>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelineMessage> {
    let event = item.as_event()?;
    let mut msg = convert_event_item(event, ctx.media_sources, ctx.own_user_id)?;
    try_enrich_from_cache(ctx.materialized, slice::from_mut(&mut msg));
    Some(msg)
}
