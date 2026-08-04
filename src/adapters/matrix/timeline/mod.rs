mod convert;
mod diff;
mod filter;
mod subscribe;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use matrix_sdk::Client;
use matrix_sdk::ruma::events::room::MediaSource;
pub(super) use subscribe::subscribe_timeline;
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::media::MediaService;
use super::profile::PronounCache;
use crate::domain::timeline::TimelineUpdate;

const ENRICH_INFLIGHT: usize = 8;

pub(super) struct TimelineContext<'a> {
    pub(super) client: &'a Client,
    pub(super) media: &'a Arc<MediaService>,
    pub(super) media_sources: &'a Arc<StdMutex<HashMap<String, MediaSource>>>,
    pub(super) pronouns: &'a Arc<PronounCache>,
    pub(super) own_user_id: Option<&'a str>,
    pub(super) first_unread: Option<&'a str>,
    pub(super) timeline_tx: &'a mpsc::Sender<TimelineUpdate>,
    pub(super) enrich: &'a EnrichmentPool,
}

pub(super) struct InflightEnrichment {
    revision: u64,
    fingerprint: u64,
    cancel: CancellationToken,
}

pub(super) type InflightEntries = HashMap<String, InflightEnrichment>;
pub(super) type InflightMap = Arc<StdMutex<InflightEntries>>;

pub(super) struct EnrichmentClaim {
    pub(super) revision: u64,
    pub(super) fingerprint: u64,
    pub(super) cancel: CancellationToken,
}

pub(super) struct EnrichmentPool {
    pub(super) tracker: TaskTracker,
    pub(super) token: CancellationToken,
    pub(super) inflight: InflightMap,
    pub(super) semaphore: Arc<Semaphore>,
    next_revision: AtomicU64,
}

impl EnrichmentPool {
    pub(super) fn new() -> Self {
        Self {
            tracker: TaskTracker::new(),
            token: CancellationToken::new(),
            inflight: Arc::new(StdMutex::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(ENRICH_INFLIGHT)),
            next_revision: AtomicU64::new(0),
        }
    }

    pub(super) fn claim(
        &self,
        unique_id: &str,
        fingerprint: u64,
        has_work: bool,
    ) -> Option<EnrichmentClaim> {
        let Ok(mut inflight) = self.inflight.lock() else {
            return None;
        };

        match inflight.remove(unique_id) {
            Some(same_revision) if same_revision.fingerprint == fingerprint => {
                inflight.insert(unique_id.to_owned(), same_revision);
                return None;
            }
            Some(superseded) => superseded.cancel.cancel(),
            None => {}
        }

        has_work.then(|| self.begin(&mut inflight, unique_id, fingerprint))
    }

    fn begin(
        &self,
        inflight: &mut InflightEntries,
        unique_id: &str,
        fingerprint: u64,
    ) -> EnrichmentClaim {
        let revision = self.next_revision.fetch_add(1, Ordering::Relaxed);
        let cancel = self.token.child_token();
        inflight.insert(
            unique_id.to_owned(),
            InflightEnrichment {
                revision,
                fingerprint,
                cancel: cancel.clone(),
            },
        );
        EnrichmentClaim {
            revision,
            fingerprint,
            cancel,
        }
    }

    pub(super) fn invalidate(&self, unique_id: &str) {
        if let Ok(mut inflight) = self.inflight.lock()
            && let Some(abandoned) = inflight.remove(unique_id)
        {
            abandoned.cancel.cancel();
        }
    }

    pub(super) fn finish(
        inflight: &StdMutex<InflightEntries>,
        unique_id: &str,
        finished_revision: u64,
    ) {
        if let Ok(mut inflight) = inflight.lock()
            && inflight
                .get(unique_id)
                .is_some_and(|current| current.revision == finished_revision)
        {
            inflight.remove(unique_id);
        }
    }
}

impl Drop for EnrichmentPool {
    fn drop(&mut self) {
        self.token.cancel();
        self.tracker.close();
    }
}
