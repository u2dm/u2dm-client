use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const MAX_CACHE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);
const ACCESS_PERSIST_INTERVAL: Duration = Duration::from_secs(60);

const RETRY_MAX_ATTEMPTS: u32 = 3;
const FAILURE_COOLDOWN: Duration = Duration::from_secs(5 * 60);

#[derive(Serialize, Deserialize)]
struct StoredEntry {
    key: String,
    path: PathBuf,
    bytes: u64,
    last_access_secs: u64,
}

struct CacheEntry {
    path: PathBuf,
    bytes: u64,
    last_access: SystemTime,
}

pub(super) struct DiskCache {
    index_path: PathBuf,
    entries: HashMap<String, CacheEntry>,
    total_bytes: u64,
    last_access_persist: SystemTime,
}

impl DiskCache {
    pub(super) fn load(media_dir: &Path) -> Self {
        let index_path = media_dir.join("index.json");
        let mut cache = Self {
            index_path,
            entries: HashMap::new(),
            total_bytes: 0,
            last_access_persist: SystemTime::now(),
        };
        cache.read_index();
        let changed = cache.prune_aged() | cache.evict_to_budget();
        if changed {
            cache.persist();
        }
        cache
    }

    fn read_index(&mut self) {
        let Ok(contents) = fs::read_to_string(&self.index_path) else {
            return;
        };
        let stored: Vec<StoredEntry> = match serde_json::from_str(&contents) {
            Ok(stored) => stored,
            Err(e) => {
                tracing::warn!("media cache index is malformed, starting empty: {e}");
                return;
            }
        };
        for entry in stored {
            if !entry.path.exists() {
                continue;
            }
            self.total_bytes = self.total_bytes.saturating_add(entry.bytes);
            self.entries.insert(
                entry.key,
                CacheEntry {
                    path: entry.path,
                    bytes: entry.bytes,
                    last_access: UNIX_EPOCH + Duration::from_secs(entry.last_access_secs),
                },
            );
        }
    }

    pub(super) fn get(&mut self, key: &str) -> Option<PathBuf> {
        let entry = self.entries.get_mut(key)?;
        if !entry.path.exists() {
            let bytes = entry.bytes;
            self.entries.remove(key);
            self.total_bytes = self.total_bytes.saturating_sub(bytes);
            return None;
        }
        let now = SystemTime::now();
        entry.last_access = now;
        let path = entry.path.clone();
        if now
            .duration_since(self.last_access_persist)
            .is_ok_and(|elapsed| elapsed >= ACCESS_PERSIST_INTERVAL)
        {
            self.last_access_persist = now;
            self.persist();
        }
        Some(path)
    }

    pub(super) fn insert(&mut self, key: &str, path: PathBuf, bytes: u64) {
        if let Some(previous) = self.entries.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(previous.bytes);
        }
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.entries.insert(
            key.to_owned(),
            CacheEntry {
                path,
                bytes,
                last_access: SystemTime::now(),
            },
        );
        self.evict_to_budget();
        self.last_access_persist = SystemTime::now();
        self.persist();
    }

    fn prune_aged(&mut self) -> bool {
        let now = SystemTime::now();
        let stale: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                now.duration_since(entry.last_access)
                    .is_ok_and(|age| age > MAX_AGE)
            })
            .map(|(key, _)| key.clone())
            .collect();
        let mut changed = false;
        for key in stale {
            self.remove_entry(&key);
            changed = true;
        }
        changed
    }

    fn evict_to_budget(&mut self) -> bool {
        let mut changed = false;
        while self.total_bytes > MAX_CACHE_BYTES {
            let Some(victim) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.remove_entry(&victim);
            changed = true;
        }
        changed
    }

    fn remove_entry(&mut self, key: &str) {
        if let Some(entry) = self.entries.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
            if let Err(e) = fs::remove_file(&entry.path) {
                tracing::debug!("failed to evict cached media {}: {e}", entry.path.display());
            }
        }
    }

    fn persist(&self) {
        let stored: Vec<StoredEntry> = self
            .entries
            .iter()
            .map(|(key, entry)| StoredEntry {
                key: key.clone(),
                path: entry.path.clone(),
                bytes: entry.bytes,
                last_access_secs: entry
                    .last_access
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or_default(),
            })
            .collect();

        let json = match serde_json::to_string(&stored) {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!("failed to serialize media cache index: {e}");
                return;
            }
        };
        let tmp = self.index_path.with_extension("tmp");
        if let Err(e) = fs::write(&tmp, json.as_bytes()) {
            tracing::warn!("failed to write media cache index: {e}");
            return;
        }
        if let Err(e) = fs::rename(&tmp, &self.index_path) {
            tracing::warn!("failed to commit media cache index: {e}");
        }
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
        self.persist();
    }
}

struct FailureRecord {
    attempts: u32,
    last_attempt: Instant,
}

#[derive(Default)]
pub(super) struct FailureTracker {
    records: HashMap<String, FailureRecord>,
}

impl FailureTracker {
    pub(super) fn should_skip(&self, key: &str) -> bool {
        self.records.get(key).is_some_and(|record| {
            record.attempts >= RETRY_MAX_ATTEMPTS
                && record.last_attempt.elapsed() < FAILURE_COOLDOWN
        })
    }

    pub(super) fn record_failure(&mut self, key: &str) {
        let record = self.records.entry(key.to_owned()).or_insert(FailureRecord {
            attempts: 0,
            last_attempt: Instant::now(),
        });
        record.attempts = record.attempts.saturating_add(1);
        record.last_attempt = Instant::now();
    }

    pub(super) fn record_success(&mut self, key: &str) {
        self.records.remove(key);
    }

    pub(super) fn clear(&mut self) {
        self.records.clear();
    }
}
