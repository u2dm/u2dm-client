use std::slice;
use std::sync::Arc;

use matrix_sdk_ui::eyeball_im::VectorDiff;
use matrix_sdk_ui::timeline::TimelineItem;

use super::TimelineContext;
use super::convert::convert_timeline_item;
use super::filter::{
    convert_and_enrich_from_cache, convert_timeline_items, is_renderable, msg_index_at,
};
use super::subscribe::spawn_enrichment_for_messages;
use crate::adapters::matrix::media::try_enrich_from_cache;
use crate::domain::models::{TimelineMessage, TimelinePatch};

fn spawn_if_needed(msg: &TimelineMessage, ctx: &TimelineContext<'_>) {
    spawn_enrichment_for_messages(slice::from_ref(msg), ctx);
}

fn apply_append(
    items: &mut Vec<Arc<TimelineItem>>,
    values: Vec<Arc<TimelineItem>>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    let mut msgs: Vec<TimelineMessage> = values
        .iter()
        .filter_map(|item| convert_timeline_item(item, ctx.media_sources, ctx.own_user_id))
        .collect();
    msgs.sort_by_key(|m| m.timestamp);
    try_enrich_from_cache(ctx.materialized, &mut msgs);
    items.extend(values);
    if msgs.is_empty() {
        return None;
    }
    spawn_enrichment_for_messages(&msgs, ctx);
    Some(TimelinePatch::Append(msgs))
}

fn apply_push_front(
    items: &mut Vec<Arc<TimelineItem>>,
    value: Arc<TimelineItem>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    let msg = convert_and_enrich_from_cache(&value, ctx);
    items.insert(0, value);
    let msg = msg?;
    spawn_if_needed(&msg, ctx);
    Some(TimelinePatch::PushFront(msg))
}

fn apply_push_back(
    items: &mut Vec<Arc<TimelineItem>>,
    value: Arc<TimelineItem>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    let msg = convert_and_enrich_from_cache(&value, ctx);
    items.push(value);
    let msg = msg?;
    spawn_if_needed(&msg, ctx);
    Some(TimelinePatch::PushBack(msg))
}

fn apply_pop_front(items: &mut Vec<Arc<TimelineItem>>) -> Option<TimelinePatch> {
    let was_renderable = items.first().is_some_and(|i| is_renderable(i));
    if !items.is_empty() {
        items.remove(0);
    }
    was_renderable.then_some(TimelinePatch::PopFront)
}

fn apply_pop_back(items: &mut Vec<Arc<TimelineItem>>) -> Option<TimelinePatch> {
    let was_renderable = items.last().is_some_and(|i| is_renderable(i));
    items.pop();
    was_renderable.then_some(TimelinePatch::PopBack)
}

fn apply_insert(
    items: &mut Vec<Arc<TimelineItem>>,
    index: usize,
    value: Arc<TimelineItem>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    let msg = convert_and_enrich_from_cache(&value, ctx);
    items.insert(index, value);
    let msg = msg?;
    let mi = msg_index_at(items, index);
    spawn_if_needed(&msg, ctx);
    Some(TimelinePatch::Insert {
        index: mi,
        message: msg,
    })
}

fn apply_set(
    items: &mut [Arc<TimelineItem>],
    index: usize,
    value: &Arc<TimelineItem>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    let old_msg = items
        .get(index)
        .and_then(|i| convert_timeline_item(i, ctx.media_sources, ctx.own_user_id));
    let old_mi = if old_msg.is_some() {
        msg_index_at(items, index)
    } else {
        0
    };

    let new_msg = convert_timeline_item(value, ctx.media_sources, ctx.own_user_id);

    if let Some(slot) = items.get_mut(index) {
        *slot = Arc::clone(value);
    }

    match (&old_msg, &new_msg) {
        (Some(old), Some(new)) if old.visually_eq(new) => None,
        (Some(_), Some(_)) => {
            let enriched = convert_and_enrich_from_cache(value, ctx)?;
            spawn_if_needed(&enriched, ctx);
            Some(TimelinePatch::Set {
                index: old_mi,
                message: enriched,
            })
        }
        (Some(_), None) => Some(TimelinePatch::Remove { index: old_mi }),
        (None, Some(_)) => {
            let mi = msg_index_at(items, index);
            let enriched = convert_and_enrich_from_cache(value, ctx)?;
            spawn_if_needed(&enriched, ctx);
            Some(TimelinePatch::Insert {
                index: mi,
                message: enriched,
            })
        }
        (None, None) => None,
    }
}

fn apply_remove(items: &mut Vec<Arc<TimelineItem>>, index: usize) -> Option<TimelinePatch> {
    let was_renderable = items.get(index).is_some_and(|i| is_renderable(i));
    let mi = if was_renderable {
        msg_index_at(items, index)
    } else {
        0
    };
    items.remove(index);
    was_renderable.then_some(TimelinePatch::Remove { index: mi })
}

fn apply_truncate(items: &mut Vec<Arc<TimelineItem>>, length: usize) -> TimelinePatch {
    let msg_length = msg_index_at(items, length);
    items.truncate(length);
    TimelinePatch::Truncate { length: msg_length }
}

fn apply_reset(
    items: &mut Vec<Arc<TimelineItem>>,
    values: Vec<Arc<TimelineItem>>,
    ctx: &TimelineContext<'_>,
) -> TimelinePatch {
    *items = values;
    let mut msgs = convert_timeline_items(items, ctx);
    try_enrich_from_cache(ctx.materialized, &mut msgs);
    spawn_enrichment_for_messages(&msgs, ctx);
    TimelinePatch::Reset(msgs)
}

pub(crate) fn diff_to_patch(
    items: &mut Vec<Arc<TimelineItem>>,
    diff: VectorDiff<Arc<TimelineItem>>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    match diff {
        VectorDiff::Append { values } => {
            let values: Vec<Arc<TimelineItem>> = values.into_iter().collect();
            apply_append(items, values, ctx)
        }
        VectorDiff::Clear => {
            items.clear();
            Some(TimelinePatch::Clear)
        }
        VectorDiff::PushFront { value } => apply_push_front(items, value, ctx),
        VectorDiff::PushBack { value } => apply_push_back(items, value, ctx),
        VectorDiff::PopFront => apply_pop_front(items),
        VectorDiff::PopBack => apply_pop_back(items),
        VectorDiff::Insert { index, value } => apply_insert(items, index, value, ctx),
        VectorDiff::Set { index, value } => apply_set(items, index, &value, ctx),
        VectorDiff::Remove { index } => apply_remove(items, index),
        VectorDiff::Truncate { length } => Some(apply_truncate(items, length)),
        VectorDiff::Reset { values } => {
            let values: Vec<Arc<TimelineItem>> = values.into_iter().collect();
            Some(apply_reset(items, values, ctx))
        }
    }
}
