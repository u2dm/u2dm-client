use std::slice;
use std::sync::Arc;

use matrix_sdk_ui::timeline::TimelineItem;

use super::TimelineContext;
use super::convert::convert_timeline_item;
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
    let mut messages: Vec<TimelineMessage> = items
        .iter()
        .filter_map(|item| convert_timeline_item(item, ctx.media_sources, ctx.own_user_id))
        .collect();
    messages.sort_by_key(|m| m.timestamp);
    messages
}

pub(super) fn convert_and_enrich_from_cache(
    item: &Arc<TimelineItem>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelineMessage> {
    let mut msg = convert_timeline_item(item, ctx.media_sources, ctx.own_user_id)?;
    try_enrich_from_cache(ctx.materialized, slice::from_mut(&mut msg));
    Some(msg)
}
