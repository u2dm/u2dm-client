use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};

use matrix_sdk::ruma::events::relation::RelationType;
use matrix_sdk::ruma::{EventId, OwnedEventId};
use matrix_sdk::serde_helpers::extract_relation;
use matrix_sdk_ui::timeline::{EventTimelineItem, TimelineItem};

#[derive(Default)]
pub(in crate::adapters::matrix) struct UndecryptedResponses {
    polls: StdMutex<HashSet<OwnedEventId>>,
}

impl UndecryptedResponses {
    pub(super) fn respond_to(&self, poll: &EventId) -> bool {
        self.polls.lock().is_ok_and(|polls| polls.contains(poll))
    }

    pub(super) fn refresh(&self, items: &[Arc<TimelineItem>]) -> Vec<OwnedEventId> {
        let current = editable_polls_with_undecrypted_references(items);
        let Ok(mut polls) = self.polls.lock() else {
            return Vec::new();
        };
        let changed = polls.symmetric_difference(&current).cloned().collect();
        *polls = current;
        changed
    }
}

fn editable_poll_id(item: &TimelineItem) -> Option<&EventId> {
    let event = item.as_event()?;
    if event.content().as_poll().is_none() || !event.is_editable() {
        return None;
    }
    event.event_id()
}

fn undecrypted_reference(event: &EventTimelineItem) -> Option<OwnedEventId> {
    event.content().as_unable_to_decrypt()?;
    match extract_relation(event.original_json()?)? {
        (RelationType::Reference, target) => Some(target),
        _ => None,
    }
}

fn editable_polls_with_undecrypted_references(
    items: &[Arc<TimelineItem>],
) -> HashSet<OwnedEventId> {
    let editable: HashSet<&EventId> = items
        .iter()
        .filter_map(|item| editable_poll_id(item))
        .collect();
    if editable.is_empty() {
        return HashSet::new();
    }
    items
        .iter()
        .filter_map(|item| undecrypted_reference(item.as_event()?))
        .filter(|target| editable.contains(&**target))
        .collect()
}
