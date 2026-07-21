use std::collections::{HashMap, HashSet};
use std::fs as std_fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{MissedTickBehavior, interval};
use tokio::{fs, task};

use crate::util::unique_tmp_path;

const MAX_CACHE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);
const FLUSH_INTERVAL: Duration = Duration::from_secs(60);
const TOUCH_COALESCE: Duration = Duration::from_secs(60);

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

#[derive(Default)]
struct Mutation {
    evicted_keys: Vec<String>,
    removed_files: Vec<PathBuf>,
}

type Index = HashMap<String, PathBuf>;

enum CacheCommand {
    Touch(String),
    Insert {
        key: String,
        path: PathBuf,
        bytes: u64,
        ack: oneshot::Sender<()>,
    },
    Clear(oneshot::Sender<()>),
}

pub(super) struct CacheHandle {
    snapshot: Arc<RwLock<Index>>,
    tx: mpsc::UnboundedSender<CacheCommand>,
    last_touch: StdMutex<HashMap<String, Instant>>,
}

impl CacheHandle {
    pub(super) fn spawn(media_dir: PathBuf) -> Self {
        let snapshot = Arc::new(RwLock::new(Index::new()));
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(CacheActor::bootstrap(media_dir, rx, Arc::clone(&snapshot)));
        Self {
            snapshot,
            tx,
            last_touch: StdMutex::new(HashMap::new()),
        }
    }

    pub(super) fn get(&self, key: &str) -> Option<PathBuf> {
        let path = self.snapshot.read().ok()?.get(key).cloned()?;
        if self.should_send_touch(key) && self.tx.send(CacheCommand::Touch(key.to_owned())).is_err()
        {
            tracing::trace!("media cache actor stopped; access not recorded");
        }
        Some(path)
    }

    fn should_send_touch(&self, key: &str) -> bool {
        let Ok(mut last) = self.last_touch.lock() else {
            return true;
        };
        let now = Instant::now();
        match last.get(key) {
            Some(sent) if now.duration_since(*sent) < TOUCH_COALESCE => false,
            _ => {
                last.insert(key.to_owned(), now);
                true
            }
        }
    }

    pub(super) async fn insert(&self, key: &str, path: PathBuf, bytes: u64) {
        let (ack_tx, ack_rx) = oneshot::channel();
        if self
            .tx
            .send(CacheCommand::Insert {
                key: key.to_owned(),
                path,
                bytes,
                ack: ack_tx,
            })
            .is_err()
        {
            tracing::debug!("media cache actor stopped; insert dropped");
            return;
        }
        if ack_rx.await.is_err() {
            tracing::trace!("media cache actor dropped insert before publishing");
        }
    }

    pub(super) async fn clear(&self) {
        if let Ok(mut last) = self.last_touch.lock() {
            last.clear();
        }
        let (ack_tx, ack_rx) = oneshot::channel();
        if self.tx.send(CacheCommand::Clear(ack_tx)).is_ok() {
            ack_rx.await.ok();
        }
    }
}

struct CacheActor {
    index_path: PathBuf,
    media_dir: PathBuf,
    entries: HashMap<String, CacheEntry>,
    total_bytes: u64,
    dirty: bool,
}

impl CacheActor {
    async fn bootstrap(
        media_dir: PathBuf,
        rx: mpsc::UnboundedReceiver<CacheCommand>,
        shared: Arc<RwLock<Index>>,
    ) {
        let actor = match task::spawn_blocking(move || CacheActor::load(media_dir)).await {
            Ok(actor) => actor,
            Err(e) => {
                tracing::error!("media cache index load task failed: {e}");
                return;
            }
        };
        if let Ok(mut guard) = shared.write() {
            *guard = actor.snapshot();
        }
        actor.run(rx, shared).await;
    }

    fn load(media_dir: PathBuf) -> Self {
        let index_path = media_dir.join("index.json");
        let mut actor = Self {
            index_path,
            media_dir,
            entries: HashMap::new(),
            total_bytes: 0,
            dirty: false,
        };
        actor.read_index();
        let mut victims = actor.prune_aged();
        victims.extend(actor.evict_to_budget());
        for (_, path) in &victims {
            if let Err(e) = std_fs::remove_file(path) {
                tracing::debug!("failed to evict cached media {}: {e}", path.display());
            }
        }
        actor.dirty = !victims.is_empty();
        actor.reconcile_orphans();
        actor
    }

    fn read_index(&mut self) {
        let Ok(contents) = std_fs::read_to_string(&self.index_path) else {
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

    fn snapshot(&self) -> Index {
        self.entries
            .iter()
            .map(|(key, entry)| (key.clone(), entry.path.clone()))
            .collect()
    }

    async fn run(
        mut self,
        mut rx: mpsc::UnboundedReceiver<CacheCommand>,
        shared: Arc<RwLock<Index>>,
    ) {
        if self.dirty {
            self.flush().await;
        }
        let mut ticker = interval(FLUSH_INTERVAL);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            tokio::select! {
                maybe = rx.recv() => {
                    let Some(cmd) = maybe else { break };
                    self.handle(cmd, &shared).await;
                }
                _ = ticker.tick() => {
                    if self.dirty {
                        self.flush().await;
                    }
                }
            }
        }
        if self.dirty {
            self.flush().await;
        }
    }

    async fn handle(&mut self, cmd: CacheCommand, shared: &RwLock<Index>) {
        match cmd {
            CacheCommand::Touch(key) => {
                if let Some(entry) = self.entries.get_mut(&key) {
                    entry.last_access = SystemTime::now();
                    self.dirty = true;
                }
            }
            CacheCommand::Insert {
                key,
                path,
                bytes,
                ack,
            } => {
                let mutation = self.insert(&key, path.clone(), bytes);
                if let Ok(mut guard) = shared.write() {
                    guard.insert(key, path);
                    for evicted in &mutation.evicted_keys {
                        guard.remove(evicted);
                    }
                }
                if ack.send(()).is_err() {
                    tracing::trace!("media cache insert requester dropped before ack");
                }
                self.delete_files(&mutation.removed_files).await;
                self.dirty = true;
            }
            CacheCommand::Clear(ack) => {
                self.entries.clear();
                self.total_bytes = 0;
                if let Ok(mut guard) = shared.write() {
                    guard.clear();
                }
                self.flush().await;
                if ack.send(()).is_err() {
                    tracing::trace!("media cache clear requester dropped before ack");
                }
            }
        }
    }

    fn insert(&mut self, key: &str, path: PathBuf, bytes: u64) -> Mutation {
        let mut mutation = Mutation::default();
        if let Some(previous) = self.entries.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(previous.bytes);
            if previous.path != path {
                mutation.removed_files.push(previous.path);
            }
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
        for (evicted_key, evicted_path) in self.evict_to_budget() {
            mutation.evicted_keys.push(evicted_key);
            mutation.removed_files.push(evicted_path);
        }
        mutation
    }

    fn prune_aged(&mut self) -> Vec<(String, PathBuf)> {
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
        let mut victims = Vec::new();
        for key in stale {
            if let Some(path) = self.remove_entry(&key) {
                victims.push((key, path));
            }
        }
        victims
    }

    fn evict_to_budget(&mut self) -> Vec<(String, PathBuf)> {
        let mut victims = Vec::new();
        while self.total_bytes > MAX_CACHE_BYTES {
            let Some(victim) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(path) = self.remove_entry(&victim) {
                victims.push((victim, path));
            }
        }
        victims
    }

    fn remove_entry(&mut self, key: &str) -> Option<PathBuf> {
        let entry = self.entries.remove(key)?;
        self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
        Some(entry.path)
    }

    async fn delete_files(&self, paths: &[PathBuf]) {
        for path in paths {
            if let Err(e) = fs::remove_file(path).await
                && e.kind() != ErrorKind::NotFound
            {
                tracing::debug!("failed to evict cached media {}: {e}", path.display());
            }
        }
    }

    async fn flush(&mut self) {
        let Some(json) = self.serialize_index() else {
            return;
        };
        if self.write_index(&json).await {
            self.dirty = false;
        }
    }

    fn serialize_index(&self) -> Option<String> {
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

        match serde_json::to_string(&stored) {
            Ok(json) => Some(json),
            Err(e) => {
                tracing::warn!("failed to serialize media cache index: {e}");
                None
            }
        }
    }

    async fn write_index(&self, json: &str) -> bool {
        match self.try_write_index(json).await {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("failed to persist media cache index: {e}");
                false
            }
        }
    }

    async fn try_write_index(&self, json: &str) -> io::Result<()> {
        if let Some(parent) = self.index_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let tmp = unique_tmp_path(&self.index_path);
        fs::write(&tmp, json.as_bytes()).await?;
        if let Err(e) = fs::rename(&tmp, &self.index_path).await {
            if let Err(cleanup_err) = fs::remove_file(&tmp).await {
                tracing::debug!("failed to remove stale media index temp: {cleanup_err}");
            }
            return Err(e);
        }
        Ok(())
    }

    fn reconcile_orphans(&self) {
        let referenced: HashSet<&Path> = self
            .entries
            .values()
            .map(|entry| entry.path.as_path())
            .collect();
        self.sweep(&self.media_dir, &referenced);
        self.sweep(&self.media_dir.join("avatars"), &referenced);
    }

    fn sweep(&self, dir: &Path, referenced: &HashSet<&Path>) {
        let Ok(read_dir) = std_fs::read_dir(dir) else {
            return;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path == self.index_path {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                continue;
            }
            if !referenced.contains(path.as_path())
                && let Err(e) = std_fs::remove_file(&path)
            {
                tracing::debug!(
                    "failed to remove orphaned cache file {}: {e}",
                    path.display()
                );
            }
        }
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
