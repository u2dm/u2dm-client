use std::mem;
use std::sync::Arc;

use matrix_sdk::ruma::EventId;
use matrix_sdk_ui::timeline::{EventTimelineItem, TimelineItem};

use super::TimelineContext;
use super::convert::convert_timeline_item;
use crate::domain::message::TimelineMessage;
use crate::domain::timeline::JumpTarget;

pub(super) struct TimelineItems {
    items: Vec<Arc<TimelineItem>>,
    messages: Vec<Option<TimelineMessage>>,
}

impl TimelineItems {
    pub(super) fn load(
        values: Vec<Arc<TimelineItem>>,
        ctx: &TimelineContext<'_>,
    ) -> (Self, Vec<TimelineMessage>) {
        let mut items = Self {
            items: Vec::new(),
            messages: Vec::new(),
        };
        let messages = items.reset(values, ctx);
        (items, messages)
    }

    pub(super) fn items(&self) -> &[Arc<TimelineItem>] {
        &self.items
    }

    pub(super) fn message_at(&self, raw_index: usize) -> Option<&TimelineMessage> {
        self.messages.get(raw_index)?.as_ref()
    }

    pub(super) fn messages_from(&self, raw_index: usize) -> impl Iterator<Item = &TimelineMessage> {
        self.messages
            .get(raw_index..)
            .into_iter()
            .flatten()
            .flatten()
    }

    pub(super) fn msg_index_at(&self, raw_index: usize) -> usize {
        self.messages
            .get(..raw_index)
            .unwrap_or(&self.messages)
            .iter()
            .filter(|message| message.is_some())
            .count()
    }

    pub(super) fn position_of_event(&self, event_id: &EventId) -> Option<usize> {
        self.items.iter().position(|item| {
            item.as_event().and_then(EventTimelineItem::event_id) == Some(event_id)
        })
    }

    pub(super) fn row_of_event(&self, event_id: &EventId) -> JumpTarget {
        let Some(raw) = self.position_of_event(event_id) else {
            return JumpTarget::NotLoaded;
        };
        if self.message_at(raw).is_some() {
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
        let messages = self.convert_and_store(&values, ctx);
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
        self.messages.clear();
    }

    pub(super) fn push_front(
        &mut self,
        value: Arc<TimelineItem>,
        message: Option<TimelineMessage>,
    ) {
        self.insert(0, value, message);
    }

    pub(super) fn push_back(&mut self, value: Arc<TimelineItem>, message: Option<TimelineMessage>) {
        self.items.push(value);
        self.messages.push(message);
    }

    pub(super) fn pop_front(&mut self) -> bool {
        if self.items.is_empty() {
            return false;
        }
        self.items.remove(0);
        self.messages.remove(0).is_some()
    }

    pub(super) fn pop_back(&mut self) -> bool {
        self.items.pop();
        self.messages.pop().flatten().is_some()
    }

    pub(super) fn insert(
        &mut self,
        index: usize,
        value: Arc<TimelineItem>,
        message: Option<TimelineMessage>,
    ) {
        self.items.insert(index, value);
        self.messages.insert(index, message);
    }

    pub(super) fn set(
        &mut self,
        index: usize,
        value: &Arc<TimelineItem>,
        message: Option<TimelineMessage>,
    ) -> Option<TimelineMessage> {
        if let Some(slot) = self.items.get_mut(index) {
            *slot = Arc::clone(value);
        }
        self.messages
            .get_mut(index)
            .and_then(|slot| mem::replace(slot, message))
    }

    pub(super) fn reconvert(
        &mut self,
        raw_index: usize,
        ctx: &TimelineContext<'_>,
    ) -> Option<TimelineMessage> {
        let message = convert_timeline_item(self.items.get(raw_index)?, ctx)?;
        if let Some(slot) = self.messages.get_mut(raw_index) {
            *slot = Some(message.clone());
        }
        Some(message)
    }

    pub(super) fn remove(&mut self, index: usize) -> bool {
        self.items.remove(index);
        self.messages.remove(index).is_some()
    }

    pub(super) fn truncate(&mut self, length: usize) {
        self.items.truncate(length);
        self.messages.truncate(length);
    }

    fn convert_and_store(
        &mut self,
        values: &[Arc<TimelineItem>],
        ctx: &TimelineContext<'_>,
    ) -> Vec<TimelineMessage> {
        let mut messages = Vec::with_capacity(values.len());
        self.messages.reserve(values.len());
        for item in values {
            let message = convert_timeline_item(item, ctx);
            if let Some(message) = &message {
                messages.push(message.clone());
            }
            self.messages.push(message);
        }
        messages
    }
}
