use std::sync::Arc;

use matrix_sdk_ui::eyeball_im::VectorDiff;
use matrix_sdk_ui::timeline::TimelineItem;

use super::TimelineContext;
use super::convert::convert_timeline_item;
use super::filter::TimelineItems;
use super::subscribe::{enrich_message, enrich_messages};
use crate::domain::models::TimelinePatch;

fn apply_append(
    items: &mut TimelineItems,
    values: Vec<Arc<TimelineItem>>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    let msgs = items.append(values, ctx);
    if msgs.is_empty() {
        return None;
    }
    enrich_messages(&msgs, ctx);
    Some(TimelinePatch::Append(msgs))
}

fn apply_push_front(
    items: &mut TimelineItems,
    value: Arc<TimelineItem>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    let msg = convert_timeline_item(&value, ctx);
    items.push_front(value, msg.is_some());
    let msg = msg?;
    enrich_message(&msg, ctx);
    Some(TimelinePatch::PushFront(msg))
}

fn apply_push_back(
    items: &mut TimelineItems,
    value: Arc<TimelineItem>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    let msg = convert_timeline_item(&value, ctx);
    items.push_back(value, msg.is_some());
    let msg = msg?;
    enrich_message(&msg, ctx);
    Some(TimelinePatch::PushBack(msg))
}

fn apply_pop_front(items: &mut TimelineItems) -> Option<TimelinePatch> {
    items.pop_front().then_some(TimelinePatch::PopFront)
}

fn apply_pop_back(items: &mut TimelineItems) -> Option<TimelinePatch> {
    items.pop_back().then_some(TimelinePatch::PopBack)
}

fn apply_insert(
    items: &mut TimelineItems,
    index: usize,
    value: Arc<TimelineItem>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    let msg = convert_timeline_item(&value, ctx);
    items.insert(index, value, msg.is_some());
    let msg = msg?;
    let mi = items.msg_index_at(index);
    enrich_message(&msg, ctx);
    Some(TimelinePatch::Insert {
        index: mi,
        message: msg,
    })
}

fn apply_set(
    items: &mut TimelineItems,
    index: usize,
    value: &Arc<TimelineItem>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    let old_msg = items
        .items()
        .get(index)
        .and_then(|item| convert_timeline_item(item, ctx));
    let new_msg = convert_timeline_item(value, ctx);

    items.set(index, value, new_msg.is_some());

    match (old_msg, new_msg) {
        (Some(old), Some(new)) if old == new => None,
        (Some(_), Some(new)) => {
            enrich_message(&new, ctx);
            Some(TimelinePatch::Set {
                index: items.msg_index_at(index),
                message: new,
            })
        }
        (Some(old), None) => {
            ctx.enrich.invalidate(&old.unique_id);
            Some(TimelinePatch::Remove {
                index: items.msg_index_at(index),
            })
        }
        (None, Some(new)) => {
            enrich_message(&new, ctx);
            Some(TimelinePatch::Insert {
                index: items.msg_index_at(index),
                message: new,
            })
        }
        (None, None) => None,
    }
}

fn apply_remove(items: &mut TimelineItems, index: usize) -> Option<TimelinePatch> {
    let mi = items.msg_index_at(index);
    items
        .remove(index)
        .then_some(TimelinePatch::Remove { index: mi })
}

fn apply_truncate(items: &mut TimelineItems, length: usize) -> TimelinePatch {
    let msg_length = items.msg_index_at(length);
    items.truncate(length);
    TimelinePatch::Truncate { length: msg_length }
}

fn apply_reset(
    items: &mut TimelineItems,
    values: Vec<Arc<TimelineItem>>,
    ctx: &TimelineContext<'_>,
) -> TimelinePatch {
    let msgs = items.reset(values, ctx);
    enrich_messages(&msgs, ctx);
    TimelinePatch::Reset(msgs)
}

pub(crate) fn diff_to_patch(
    items: &mut TimelineItems,
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
