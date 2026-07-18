use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex as StdMutex};

use matrix_sdk::Client;
use matrix_sdk::ruma::OwnedUserId;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use tokio::sync::OnceCell;

const PRONOUNS_FIELD: &str = "m.pronouns";
const PRONOUNS_FIELD_UNSTABLE: &str = "io.fsky.nyx.pronouns";
const MAX_CACHED_SENDERS: usize = 512;

#[derive(Deserialize)]
struct PronounSet {
    summary: String,
}

#[derive(Default)]
struct PronounStore {
    senders: HashMap<String, Arc<OnceCell<Vec<String>>>>,
    order: VecDeque<String>,
}

#[derive(Default)]
pub(super) struct PronounCache {
    store: StdMutex<PronounStore>,
}

impl PronounCache {
    pub(super) fn resolved(&self, sender: &str) -> Vec<String> {
        let Ok(store) = self.store.lock() else {
            return Vec::new();
        };
        store
            .senders
            .get(sender)
            .and_then(|cell| cell.get().cloned())
            .unwrap_or_default()
    }

    pub(super) fn is_resolved(&self, sender: &str) -> bool {
        self.store.lock().is_ok_and(|store| {
            store
                .senders
                .get(sender)
                .is_some_and(|cell| cell.initialized())
        })
    }

    pub(super) async fn resolve(&self, client: &Client, sender: &str) -> Vec<String> {
        let Some(cell) = self.cell(sender) else {
            return Vec::new();
        };
        cell.get_or_init(|| fetch_pronouns(client, sender))
            .await
            .clone()
    }

    fn cell(&self, sender: &str) -> Option<Arc<OnceCell<Vec<String>>>> {
        let mut store = self.store.lock().ok()?;
        if let Some(cell) = store.senders.get(sender) {
            return Some(Arc::clone(cell));
        }
        let cell = Arc::new(OnceCell::new());
        store.senders.insert(sender.to_owned(), Arc::clone(&cell));
        store.order.push_back(sender.to_owned());
        while store.order.len() > MAX_CACHED_SENDERS {
            if let Some(evicted) = store.order.pop_front() {
                store.senders.remove(&evicted);
            }
        }
        Some(cell)
    }
}

async fn fetch_pronouns(client: &Client, sender: &str) -> Vec<String> {
    let Ok(user_id) = OwnedUserId::try_from(sender) else {
        return Vec::new();
    };

    let profile = match client.account().fetch_user_profile_of(&user_id).await {
        Ok(profile) => profile,
        Err(e) => {
            tracing::debug!("pronoun lookup failed for {sender}: {e}");
            return Vec::new();
        }
    };

    profile
        .get(PRONOUNS_FIELD)
        .or_else(|| profile.get(PRONOUNS_FIELD_UNSTABLE))
        .map_or_else(Vec::new, summaries)
}

fn summaries(value: &JsonValue) -> Vec<String> {
    if let Some(single) = value.as_str() {
        return vec![single.to_owned()];
    }
    match serde_json::from_value::<Vec<PronounSet>>(value.clone()) {
        Ok(sets) => sets.into_iter().map(|set| set.summary).collect(),
        Err(e) => {
            tracing::debug!("pronouns field is not a known shape: {e}");
            Vec::new()
        }
    }
}
