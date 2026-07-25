use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use matrix_sdk::Client;
use matrix_sdk::ruma::OwnedUserId;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use tokio::sync::OnceCell;

const PRONOUNS_FIELD: &str = "m.pronouns";
const PRONOUNS_FIELD_UNSTABLE: &str = "io.fsky.nyx.pronouns";
const MAX_CACHED_SENDERS: usize = 512;
const FAILURE_COOLDOWN: Duration = Duration::from_mins(5);

#[derive(Deserialize)]
struct PronounSet {
    summary: String,
}

struct FetchFailed;

struct SenderEntry {
    cell: Arc<OnceCell<Vec<String>>>,
    failed_at: Option<Instant>,
}

impl SenderEntry {
    fn value(&self) -> Option<Vec<String>> {
        self.cell.get().cloned()
    }

    fn in_cooldown(&self) -> bool {
        self.failed_at
            .is_some_and(|failed_at| failed_at.elapsed() < FAILURE_COOLDOWN)
    }
}

enum Acquire {
    Resolved(Vec<String>),
    Skip,
    Fetch(Arc<OnceCell<Vec<String>>>),
}

#[derive(Default)]
struct PronounStore {
    senders: HashMap<String, SenderEntry>,
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
            .and_then(SenderEntry::value)
            .unwrap_or_default()
    }

    pub(super) fn needs_fetch(&self, sender: &str) -> bool {
        self.store.lock().is_ok_and(|store| {
            store
                .senders
                .get(sender)
                .is_none_or(|entry| !entry.cell.initialized() && !entry.in_cooldown())
        })
    }

    pub(super) async fn resolve(&self, client: &Client, sender: &str) -> Vec<String> {
        let cell = match self.acquire(sender) {
            Acquire::Resolved(resolved) => return resolved,
            Acquire::Skip => return Vec::new(),
            Acquire::Fetch(cell) => cell,
        };
        if let Ok(resolved) = cell
            .get_or_try_init(|| fetch_pronouns(client, sender))
            .await
        {
            resolved.clone()
        } else {
            self.record_failure(sender);
            Vec::new()
        }
    }

    fn acquire(&self, sender: &str) -> Acquire {
        let Ok(mut store) = self.store.lock() else {
            return Acquire::Skip;
        };
        if let Some(entry) = store.senders.get(sender) {
            if let Some(resolved) = entry.value() {
                return Acquire::Resolved(resolved);
            }
            if entry.in_cooldown() {
                return Acquire::Skip;
            }
            return Acquire::Fetch(Arc::clone(&entry.cell));
        }
        let cell = Arc::new(OnceCell::new());
        store.senders.insert(
            sender.to_owned(),
            SenderEntry {
                cell: Arc::clone(&cell),
                failed_at: None,
            },
        );
        store.order.push_back(sender.to_owned());
        while store.order.len() > MAX_CACHED_SENDERS {
            if let Some(evicted) = store.order.pop_front() {
                store.senders.remove(&evicted);
            }
        }
        Acquire::Fetch(cell)
    }

    fn record_failure(&self, sender: &str) {
        if let Ok(mut store) = self.store.lock()
            && let Some(entry) = store.senders.get_mut(sender)
        {
            entry.failed_at = Some(Instant::now());
        }
    }
}

async fn fetch_pronouns(client: &Client, sender: &str) -> Result<Vec<String>, FetchFailed> {
    let Ok(user_id) = OwnedUserId::try_from(sender) else {
        return Ok(Vec::new());
    };

    let profile = match client.account().fetch_user_profile_of(&user_id).await {
        Ok(profile) => profile,
        Err(e) => {
            tracing::debug!("pronoun lookup failed for {sender}: {e}");
            return Err(FetchFailed);
        }
    };

    Ok(profile
        .get(PRONOUNS_FIELD)
        .or_else(|| profile.get(PRONOUNS_FIELD_UNSTABLE))
        .map_or_else(Vec::new, summaries))
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
