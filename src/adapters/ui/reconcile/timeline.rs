use std::collections::HashMap;
use std::mem;

use slint::{Model, VecModel};

use crate::adapters::ui::decode::forget_all_media_needs;
use crate::adapters::ui::richtext::forget_styled_bodies;
use crate::adapters::ui::session::with_session;
use crate::adapters::ui::splice_model::SpliceModel;
use crate::adapters::ui::spoilers::forget_spoiler_rows;
use crate::domain::message::TimelineMessage;
use crate::domain::timeline::{EnrichmentDelta, TimelinePatch};

#[derive(Default)]
pub struct TimelineIndex {
    row_of: HashMap<String, usize>,
    fingerprint_of: HashMap<String, u64>,
}

impl TimelineIndex {
    fn clear(&mut self) {
        self.row_of.clear();
        self.fingerprint_of.clear();
    }

    fn reset(&mut self, messages: &[TimelineMessage]) {
        self.clear();
        self.extend_from(0, messages);
    }

    fn extend_from(&mut self, base: usize, messages: &[TimelineMessage]) {
        for (offset, message) in messages.iter().enumerate() {
            self.remember(base.saturating_add(offset), message);
        }
    }

    fn remember(&mut self, row: usize, message: &TimelineMessage) {
        self.row_of.insert(message.unique_id.clone(), row);
        self.fingerprint_of
            .insert(message.unique_id.clone(), message.enrichment_fingerprint());
    }

    fn forget(&mut self, unique_id: &str) {
        self.row_of.remove(unique_id);
        self.fingerprint_of.remove(unique_id);
    }

    fn spliced_at(&mut self, row: usize, messages: &[TimelineMessage]) {
        let count = messages.len();
        for existing in self.row_of.values_mut() {
            if *existing >= row {
                *existing = existing.saturating_add(count);
            }
        }
        self.extend_from(row, messages);
    }

    fn replaced_at(&mut self, row: usize, previous: Option<&str>, message: &TimelineMessage) {
        if let Some(previous) = previous.filter(|id| *id != message.unique_id) {
            self.forget(previous);
        }
        self.remember(row, message);
    }

    fn removed_at(&mut self, row: usize, unique_id: Option<&str>) {
        if let Some(unique_id) = unique_id {
            self.forget(unique_id);
        }
        for existing in self.row_of.values_mut() {
            if *existing > row {
                *existing = existing.saturating_sub(1);
            }
        }
    }

    fn truncated_to(&mut self, length: usize) {
        let Self {
            row_of,
            fingerprint_of,
        } = self;
        row_of.retain(|_, row| *row < length);
        fingerprint_of.retain(|unique_id, _| row_of.contains_key(unique_id));
    }

    fn holds_revision(&self, delta: &EnrichmentDelta) -> bool {
        self.fingerprint_of.get(&delta.unique_id) == Some(&delta.fingerprint)
    }

    fn shows_same_media(&self, unique_id: &str, message: &TimelineMessage) -> bool {
        unique_id == message.unique_id
            && self.fingerprint_of.get(unique_id) == Some(&message.enrichment_fingerprint())
    }

    fn row_for(&self, delta: &EnrichmentDelta) -> Option<usize> {
        self.row_of
            .get(&delta.unique_id)
            .copied()
            .filter(|_| self.holds_revision(delta))
    }
}

pub fn timeline_row_of(unique_id: &str) -> Option<usize> {
    with_session(|session| session.timeline_index.row_of.get(unique_id).copied())
}

struct TimelineOps<'a, T> {
    convert: &'a dyn Fn(&TimelineMessage) -> T,
    keep_shown: &'a dyn Fn(&T, &mut T),
    enrich: &'a dyn Fn(&mut T, &EnrichmentDelta),
    entry_id: &'a dyn Fn(&T) -> &str,
}

pub fn apply_timeline_patch<T: Clone + 'static>(
    model: &SpliceModel<T>,
    patch: TimelinePatch,
    convert: &dyn Fn(&TimelineMessage) -> T,
    keep_shown: &dyn Fn(&T, &mut T),
    enrich: &dyn Fn(&mut T, &EnrichmentDelta),
    entry_id: &dyn Fn(&T) -> &str,
) {
    let ops = TimelineOps {
        convert,
        keep_shown,
        enrich,
        entry_id,
    };
    let mut index = with_session(|session| mem::take(&mut session.timeline_index));
    apply_patch(model, patch, &mut index, &ops);
    with_session(|session| session.timeline_index = index);
}

fn forget_row_caches() {
    forget_all_media_needs();
    forget_styled_bodies();
    forget_spoiler_rows();
}

fn apply_patch<T: Clone + 'static>(
    model: &SpliceModel<T>,
    patch: TimelinePatch,
    index: &mut TimelineIndex,
    ops: &TimelineOps<'_, T>,
) {
    let TimelineOps {
        convert,
        enrich,
        entry_id,
        ..
    } = *ops;
    let before = model.row_count();
    tracing::debug!(
        patch = patch.label(),
        model_rows_before = before,
        "apply_timeline_patch"
    );
    match patch {
        TimelinePatch::Reset(messages) => {
            forget_row_caches();
            index.reset(&messages);
            let entries: Vec<T> = messages.iter().map(convert).collect();
            model.set_vec(entries);
        }
        TimelinePatch::Append(messages) => splice_rows(model, before, &messages, index, convert),
        TimelinePatch::PushFront(m) => splice_rows(model, 0, &[m], index, convert),
        TimelinePatch::PushBack(m) => splice_rows(model, before, &[m], index, convert),
        TimelinePatch::Insert { index: at, message } => {
            splice_rows(model, at.min(before), &[message], index, convert);
        }
        TimelinePatch::Set { index: at, message } => {
            set_row(model, at, before, index, &message, ops);
        }
        TimelinePatch::Remove { index: at } => remove_row(model, at, before, index, entry_id),
        TimelinePatch::PopFront => remove_row(model, 0, before, index, entry_id),
        TimelinePatch::PopBack => {
            remove_row(model, before.saturating_sub(1), before, index, entry_id);
        }
        TimelinePatch::Truncate { length } => truncate_rows(model, length, index),
        TimelinePatch::Clear => {
            forget_row_caches();
            index.clear();
            model.set_vec(Vec::new());
        }
        TimelinePatch::Batch(patches) => apply_batch(model, patches, index, ops),
        TimelinePatch::Enrich(delta) => enrich_target(model, &delta, index, enrich, entry_id),
    }
    tracing::debug!(
        model_rows_after = model.row_count(),
        "apply_timeline_patch done"
    );
}

fn id_at<T: Clone + 'static>(
    model: &SpliceModel<T>,
    row: usize,
    entry_id: &dyn Fn(&T) -> &str,
) -> Option<String> {
    model.row_data(row).map(|entry| entry_id(&entry).to_owned())
}

fn splice_rows<T: Clone + 'static>(
    model: &SpliceModel<T>,
    row: usize,
    messages: &[TimelineMessage],
    index: &mut TimelineIndex,
    convert: &dyn Fn(&TimelineMessage) -> T,
) {
    if messages.is_empty() {
        return;
    }
    if row < model.row_count() {
        index.spliced_at(row, messages);
    } else {
        index.extend_from(row, messages);
    }
    model.insert_rows(row, messages.iter().map(convert).collect());
}

#[derive(Default)]
struct EdgeInsertions {
    newest_first_at_front: Vec<TimelineMessage>,
    at_back: Vec<TimelineMessage>,
}

impl EdgeInsertions {
    fn absorb(&mut self, patch: TimelinePatch) -> Option<TimelinePatch> {
        match patch {
            TimelinePatch::PushFront(message) | TimelinePatch::Insert { index: 0, message } => {
                self.newest_first_at_front.push(message);
            }
            TimelinePatch::PushBack(message) => self.at_back.push(message),
            TimelinePatch::Append(messages) => self.at_back.extend(messages),
            other => return Some(other),
        }
        None
    }

    fn flush<T: Clone + 'static>(
        &mut self,
        model: &SpliceModel<T>,
        index: &mut TimelineIndex,
        convert: &dyn Fn(&TimelineMessage) -> T,
    ) {
        let mut front = mem::take(&mut self.newest_first_at_front);
        front.reverse();
        splice_rows(model, 0, &front, index, convert);
        let back = mem::take(&mut self.at_back);
        splice_rows(model, model.row_count(), &back, index, convert);
    }
}

fn apply_batch<T: Clone + 'static>(
    model: &SpliceModel<T>,
    patches: Vec<TimelinePatch>,
    index: &mut TimelineIndex,
    ops: &TimelineOps<'_, T>,
) {
    let mut edges = EdgeInsertions::default();
    for patch in patches {
        if let Some(positional) = edges.absorb(patch) {
            edges.flush(model, index, ops.convert);
            apply_patch(model, positional, index, ops);
        }
    }
    edges.flush(model, index, ops.convert);
}

fn set_row<T: Clone + 'static>(
    model: &SpliceModel<T>,
    row: usize,
    row_count: usize,
    index: &mut TimelineIndex,
    message: &TimelineMessage,
    ops: &TimelineOps<'_, T>,
) {
    if row >= row_count {
        return;
    }
    let shown = model.row_data(row);
    let shown_id = shown.as_ref().map(|entry| (ops.entry_id)(entry).to_owned());
    let same_media = shown_id
        .as_deref()
        .is_some_and(|id| index.shows_same_media(id, message));
    index.replaced_at(row, shown_id.as_deref(), message);
    let mut rebuilt = (ops.convert)(message);
    if same_media && let Some(shown) = &shown {
        (ops.keep_shown)(shown, &mut rebuilt);
    }
    model.set_row_data(row, rebuilt);
}

fn remove_row<T: Clone + 'static>(
    model: &SpliceModel<T>,
    row: usize,
    row_count: usize,
    index: &mut TimelineIndex,
    entry_id: &dyn Fn(&T) -> &str,
) {
    if row >= row_count {
        return;
    }
    index.removed_at(row, id_at(model, row, entry_id).as_deref());
    model.remove(row);
}

fn truncate_rows<T: Clone + 'static>(
    model: &SpliceModel<T>,
    length: usize,
    index: &mut TimelineIndex,
) {
    index.truncated_to(length);
    model.truncate(length);
}

fn enrich_target<T: Clone + 'static>(
    model: &SpliceModel<T>,
    delta: &EnrichmentDelta,
    index: &TimelineIndex,
    enrich: &dyn Fn(&mut T, &EnrichmentDelta),
    entry_id: &dyn Fn(&T) -> &str,
) {
    let Some(row) = index.row_for(delta) else {
        tracing::debug!(
            unique_id = delta.unique_id,
            "dropped an enrichment delta with no live row at its revision"
        );
        return;
    };
    enrich_row(model, row, delta, enrich, entry_id);
}

fn enrich_row<T: Clone + 'static>(
    model: &SpliceModel<T>,
    row: usize,
    delta: &EnrichmentDelta,
    enrich: &dyn Fn(&mut T, &EnrichmentDelta),
    entry_id: &dyn Fn(&T) -> &str,
) {
    let Some(mut entry) = model.row_data(row) else {
        return;
    };
    if entry_id(&entry) != delta.unique_id.as_str() {
        tracing::warn!(
            row,
            unique_id = delta.unique_id,
            "the timeline index disagrees with the model, dropping an enrichment delta"
        );
        return;
    }
    enrich(&mut entry, delta);
    model.set_row_data(row, entry);
}

pub fn reorder_rows<T: Clone + 'static>(model: &VecModel<T>, from: usize, to: usize) {
    if from < model.row_count() && to < model.row_count() {
        let entry = model.remove(from);
        model.insert(to, entry);
    }
}
