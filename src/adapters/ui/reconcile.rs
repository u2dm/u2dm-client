use std::collections::HashMap;

use slint::{Model, VecModel};

use super::decode::forget_all_media_needs;
use crate::domain::models::{EnrichmentDelta, Room, TimelineMessage, TimelinePatch};

pub fn apply_timeline_patch<T: Clone + 'static>(
    model: &VecModel<T>,
    patch: TimelinePatch,
    convert: &dyn Fn(&TimelineMessage) -> T,
    enrich: &dyn Fn(&mut T, &EnrichmentDelta),
    entry_id: &dyn Fn(&T) -> String,
) {
    let before = model.row_count();
    tracing::debug!(
        patch = patch.label(),
        model_rows_before = before,
        "apply_timeline_patch"
    );
    match patch {
        TimelinePatch::Reset(messages) => {
            forget_all_media_needs();
            let entries: Vec<T> = messages.iter().map(convert).collect();
            model.set_vec(entries);
        }
        TimelinePatch::Append(messages) => {
            for m in &messages {
                model.push(convert(m));
            }
        }
        TimelinePatch::PushFront(m) => {
            model.insert(0, convert(&m));
        }
        TimelinePatch::PushBack(m) => {
            model.push(convert(&m));
        }
        TimelinePatch::Insert { index, message } => {
            let idx = index.min(model.row_count());
            model.insert(idx, convert(&message));
        }
        TimelinePatch::Set { index, message } => {
            if index < model.row_count() {
                model.set_row_data(index, convert(&message));
            }
        }
        TimelinePatch::Remove { index } => {
            if index < model.row_count() {
                model.remove(index);
            }
        }
        TimelinePatch::PopFront => {
            if model.row_count() > 0 {
                model.remove(0);
            }
        }
        TimelinePatch::PopBack => {
            let count = model.row_count();
            if count > 0 {
                model.remove(count - 1);
            }
        }
        TimelinePatch::Truncate { length } => {
            while model.row_count() > length {
                model.remove(model.row_count() - 1);
            }
        }
        TimelinePatch::Clear => {
            forget_all_media_needs();
            model.set_vec(Vec::new());
        }
        TimelinePatch::Batch(patches) => {
            apply_batch(model, patches, convert, enrich, entry_id);
        }
        TimelinePatch::Enrich(delta) => {
            for i in 0..model.row_count() {
                if let Some(entry) = model.row_data(i)
                    && entry_id(&entry) == delta.unique_id
                {
                    let mut updated = entry;
                    enrich(&mut updated, &delta);
                    model.set_row_data(i, updated);
                    break;
                }
            }
        }
    }
    tracing::debug!(
        model_rows_after = model.row_count(),
        "apply_timeline_patch done"
    );
}

fn apply_batch<T: Clone + 'static>(
    model: &VecModel<T>,
    patches: Vec<TimelinePatch>,
    convert: &dyn Fn(&TimelineMessage) -> T,
    enrich: &dyn Fn(&mut T, &EnrichmentDelta),
    entry_id: &dyn Fn(&T) -> String,
) {
    for p in patches {
        apply_timeline_patch(model, p, convert, enrich, entry_id);
    }
}

pub fn reorder_rows<T: Clone + 'static>(model: &VecModel<T>, from: usize, to: usize) {
    if from < model.row_count() && to < model.row_count() {
        let entry = model.remove(from);
        model.insert(to, entry);
    }
}

pub fn apply_rooms<T: Clone + PartialEq + 'static>(
    model: &VecModel<T>,
    rooms: &[Room],
    convert: &dyn Fn(&Room) -> T,
    get_id: &dyn Fn(&T) -> &str,
) {
    apply_reconcile(model, rooms, &|r| r.id.as_ref(), convert, get_id);
}

pub fn apply_reconcile<S, T: Clone + PartialEq + 'static>(
    model: &VecModel<T>,
    items: &[S],
    source_id: &dyn Fn(&S) -> &str,
    convert: &dyn Fn(&S) -> T,
    get_id: &dyn Fn(&T) -> &str,
) {
    let new_ids: HashMap<&str, usize> = items
        .iter()
        .enumerate()
        .map(|(i, item)| (source_id(item), i))
        .collect();

    let mut i = 0;
    while i < model.row_count() {
        let keep = model
            .row_data(i)
            .is_some_and(|entry| new_ids.contains_key(get_id(&entry)));

        if keep {
            i += 1;
        } else {
            model.remove(i);
        }
    }

    for idx in 0..items.len() {
        let Some(item) = items.get(idx) else { continue };
        let new_entry = convert(item);

        if idx < model.row_count() {
            let same_id = model
                .row_data(idx)
                .is_some_and(|entry| get_id(&entry) == source_id(item));

            if same_id {
                if model.row_data(idx).as_ref() != Some(&new_entry) {
                    model.set_row_data(idx, new_entry);
                }
            } else {
                model.insert(idx, new_entry);
            }
        } else {
            model.push(new_entry);
        }
    }

    while model.row_count() > items.len() {
        model.remove(model.row_count() - 1);
    }
}
