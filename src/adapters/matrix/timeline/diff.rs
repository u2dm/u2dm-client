use std::collections::HashMap;
use std::sync::Arc;

use matrix_sdk_ui::eyeball_im::VectorDiff;
use matrix_sdk_ui::timeline::TimelineItem;

use super::TimelineContext;
use super::convert::convert_timeline_item;
use super::filter::TimelineItems;
use super::subscribe::{enrich_message, enrich_messages};
use crate::domain::message::TimelineMessage;
use crate::domain::timeline::TimelinePatch;

fn apply_append(
    items: &mut TimelineItems,
    values: Vec<Arc<TimelineItem>>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    let first_new = items.items().len();
    let msgs = items.append(values, ctx);
    if msgs.is_empty() {
        return None;
    }
    enrich_messages(items.rendered_from(first_new), ctx);
    Some(TimelinePatch::Append(msgs))
}

fn convert_and_enrich(value: &TimelineItem, ctx: &TimelineContext<'_>) -> Option<TimelineMessage> {
    let msg = convert_timeline_item(value, ctx)?;
    enrich_message(value, &msg, ctx);
    Some(msg)
}

fn apply_push_front(
    items: &mut TimelineItems,
    value: Arc<TimelineItem>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    let msg = convert_and_enrich(&value, ctx);
    items.push_front(value, msg.clone());
    msg.map(TimelinePatch::PushFront)
}

fn apply_push_back(
    items: &mut TimelineItems,
    value: Arc<TimelineItem>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    let msg = convert_and_enrich(&value, ctx);
    items.push_back(value, msg.clone());
    msg.map(TimelinePatch::PushBack)
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
    let msg = convert_and_enrich(&value, ctx);
    items.insert(index, value, msg.clone());
    let msg = msg?;
    let mi = items.msg_index_at(index);
    Some(TimelinePatch::Insert {
        index: mi,
        message: msg,
    })
}

fn with_current_pronouns(message: TimelineMessage, ctx: &TimelineContext<'_>) -> TimelineMessage {
    TimelineMessage {
        sender_pronouns: ctx.pronouns.resolved(&message.sender),
        ..message
    }
}

fn apply_set(
    items: &mut TimelineItems,
    index: usize,
    value: &Arc<TimelineItem>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    let converted = convert_timeline_item(value, ctx);
    let old_msg = items
        .set(index, value, converted)
        .map(|old| with_current_pronouns(old, ctx));

    match (old_msg, items.message_at(index)) {
        (Some(old), Some(new)) if old == *new => None,
        (Some(_), Some(new)) => {
            enrich_message(value, new, ctx);
            Some(TimelinePatch::Set {
                index: items.msg_index_at(index),
                message: new.clone(),
            })
        }
        (Some(old), None) => {
            ctx.enrich.invalidate(&old.unique_id);
            Some(TimelinePatch::Remove {
                index: items.msg_index_at(index),
            })
        }
        (None, Some(new)) => {
            enrich_message(value, new, ctx);
            Some(TimelinePatch::Insert {
                index: items.msg_index_at(index),
                message: new.clone(),
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
    enrich_messages(items.rendered_from(0), ctx);
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

struct ReadStamp {
    row: usize,
    message: TimelineMessage,
    carried: bool,
}

pub(crate) fn stamp_read_marks(
    items: &mut TimelineItems,
    batch: &mut Vec<TimelinePatch>,
    ctx: &TimelineContext<'_>,
) {
    let mut stamps: HashMap<String, ReadStamp> = items
        .restamp_read_by(ctx.own_user_id)
        .into_iter()
        .map(|(row, message)| {
            let stamp = ReadStamp {
                row,
                message,
                carried: false,
            };
            (stamp.message.unique_id.clone(), stamp)
        })
        .collect();
    if stamps.is_empty() {
        return;
    }
    for patch in batch.iter_mut() {
        patch.visit_messages_mut(&mut |message| {
            if let Some(stamp) = stamps.get_mut(&message.unique_id) {
                message.read_by.clone_from(&stamp.message.read_by);
                stamp.carried = true;
            }
        });
    }
    let mut uncarried: Vec<ReadStamp> = stamps
        .into_values()
        .filter(|stamp| !stamp.carried)
        .collect();
    uncarried.sort_by_key(|stamp| stamp.row);
    tracing::debug!(rows = uncarried.len(), "read marks moved");
    batch.extend(uncarried.into_iter().map(|stamp| TimelinePatch::Set {
        index: stamp.row,
        message: stamp.message,
    }));
}
