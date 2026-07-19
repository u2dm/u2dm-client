mod convert;
mod diff;
mod filter;
mod subscribe;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};

use matrix_sdk::Client;
use matrix_sdk::ruma::events::room::MediaSource;
pub(super) use subscribe::subscribe_timeline;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::media::MediaService;
use super::profile::PronounCache;
use crate::domain::models::TimelineUpdate;

pub(super) struct TimelineContext<'a> {
    pub(super) client: &'a Client,
    pub(super) media: &'a Arc<MediaService>,
    pub(super) media_sources: &'a Arc<StdMutex<HashMap<String, MediaSource>>>,
    pub(super) pronouns: &'a Arc<PronounCache>,
    pub(super) own_user_id: Option<&'a str>,
    pub(super) timeline_tx: &'a mpsc::Sender<TimelineUpdate>,
    pub(super) enrich: &'a EnrichmentPool,
}

pub(super) struct EnrichmentPool {
    pub(super) tracker: TaskTracker,
    pub(super) token: CancellationToken,
    pub(super) inflight: Arc<StdMutex<HashSet<String>>>,
}

impl EnrichmentPool {
    pub(super) fn new() -> Self {
        Self {
            tracker: TaskTracker::new(),
            token: CancellationToken::new(),
            inflight: Arc::new(StdMutex::new(HashSet::new())),
        }
    }
}

impl Drop for EnrichmentPool {
    fn drop(&mut self) {
        self.token.cancel();
        self.tracker.close();
    }
}
