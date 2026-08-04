use std::sync::Arc;

use matrix_sdk::ruma::EventId;
use matrix_sdk_ui::timeline::{EventTimelineItem, TimelineItem};

use super::TimelineContext;
use super::convert::convert_timeline_item;
use crate::domain::message::TimelineMessage;
use crate::domain::timeline::JumpTarget;

pub(super) struct TimelineItems {
    items: Vec<Arc<TimelineItem>>,
    renderable: Vec<bool>,
}

impl TimelineItems {
    pub(super) fn load(
        values: Vec<Arc<TimelineItem>>,
        ctx: &TimelineContext<'_>,
    ) -> (Self, Vec<TimelineMessage>) {
        let mut items = Self {
            items: Vec::new(),
            renderable: Vec::new(),
        };
        let messages = items.reset(values, ctx);
        (items, messages)
    }

    pub(super) fn items(&self) -> &[Arc<TimelineItem>] {
        &self.items
    }

    pub(super) fn msg_index_at(&self, raw_index: usize) -> usize {
        self.renderable
            .get(..raw_index)
            .unwrap_or(&self.renderable)
            .iter()
            .filter(|renderable| **renderable)
            .count()
    }

    pub(super) fn row_of_event(&self, event_id: &EventId) -> JumpTarget {
        let Some(raw) = self.items.iter().position(|item| {
            item.as_event().and_then(EventTimelineItem::event_id) == Some(event_id)
        }) else {
            return JumpTarget::NotLoaded;
        };
        if self.renderable.get(raw).is_some_and(|renders| *renders) {
            JumpTarget::Row(self.msg_index_at(raw))
        } else {
            JumpTarget::NotRenderable
        }
    }

    pub(super) fn append(
        &mut self,
        values: Vec<Arc<TimelineItem>>,
        ctx: &TimelineContext<'_>,
    ) -> Vec<TimelineMessage> {
        let messages = self.convert_and_flag(&values, ctx);
        self.items.extend(values);
        messages
    }

    pub(super) fn reset(
        &mut self,
        values: Vec<Arc<TimelineItem>>,
        ctx: &TimelineContext<'_>,
    ) -> Vec<TimelineMessage> {
        self.clear();
        self.append(values, ctx)
    }

    pub(super) fn clear(&mut self) {
        self.items.clear();
        self.renderable.clear();
    }

    pub(super) fn push_front(&mut self, value: Arc<TimelineItem>, renderable: bool) {
        self.insert(0, value, renderable);
    }

    pub(super) fn push_back(&mut self, value: Arc<TimelineItem>, renderable: bool) {
        self.items.push(value);
        self.renderable.push(renderable);
    }

    pub(super) fn pop_front(&mut self) -> bool {
        if self.items.is_empty() {
            return false;
        }
        self.items.remove(0);
        self.renderable.remove(0)
    }

    pub(super) fn pop_back(&mut self) -> bool {
        self.items.pop();
        self.renderable.pop().unwrap_or(false)
    }

    pub(super) fn insert(&mut self, index: usize, value: Arc<TimelineItem>, renderable: bool) {
        self.items.insert(index, value);
        self.renderable.insert(index, renderable);
    }

    pub(super) fn set(&mut self, index: usize, value: &Arc<TimelineItem>, renderable: bool) {
        if let Some(slot) = self.items.get_mut(index) {
            *slot = Arc::clone(value);
        }
        if let Some(slot) = self.renderable.get_mut(index) {
            *slot = renderable;
        }
    }

    pub(super) fn remove(&mut self, index: usize) -> bool {
        self.items.remove(index);
        self.renderable.remove(index)
    }

    pub(super) fn truncate(&mut self, length: usize) {
        self.items.truncate(length);
        self.renderable.truncate(length);
    }

    fn convert_and_flag(
        &mut self,
        values: &[Arc<TimelineItem>],
        ctx: &TimelineContext<'_>,
    ) -> Vec<TimelineMessage> {
        let mut messages = Vec::with_capacity(values.len());
        self.renderable.reserve(values.len());
        for item in values {
            let message = convert_timeline_item(item, ctx);
            self.renderable.push(message.is_some());
            if let Some(message) = message {
                messages.push(message);
            }
        }
        messages
    }
}
