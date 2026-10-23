use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use tokio::time::Instant;

use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::media::MediaSource;

pub(super) const RECEIVE_WAIT: Duration = Duration::from_secs(15);
pub(super) const RECEIVE_CHECK: Duration = Duration::from_secs(3);

pub(super) struct Wanted {
    pub(super) sid: String,
    pub(super) identity: MediaIdentity,
    pub(super) source: MediaSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Change {
    pub(super) identity: MediaIdentity,
    pub(super) source: MediaSource,
    pub(super) missing: bool,
}

#[derive(Default)]
pub(super) struct Reception {
    waiting: HashMap<String, Instant>,
    missing: BTreeMap<String, (MediaIdentity, MediaSource)>,
}

impl Reception {
    pub(super) fn check(&mut self, now: Instant, wanted: &[Wanted]) -> Vec<Change> {
        let still_wanted: HashSet<&str> = wanted.iter().map(|track| track.sid.as_str()).collect();
        self.waiting
            .retain(|sid, _| still_wanted.contains(sid.as_str()));
        let mut changes = self.arrived(&still_wanted);
        for track in self.overdue(now, wanted) {
            self.waiting.remove(&track.sid);
            let first = !self.reports(&track.identity, track.source);
            self.missing
                .insert(track.sid.clone(), (track.identity.clone(), track.source));
            if first {
                changes.push(Change {
                    identity: track.identity.clone(),
                    source: track.source,
                    missing: true,
                });
            }
        }
        changes
    }

    pub(super) fn restart(&mut self) {
        self.waiting.clear();
    }

    fn arrived(&mut self, still_wanted: &HashSet<&str>) -> Vec<Change> {
        let gone: Vec<String> = self
            .missing
            .keys()
            .filter(|sid| !still_wanted.contains(sid.as_str()))
            .cloned()
            .collect();
        let mut changes = Vec::new();
        for sid in gone {
            if let Some((identity, source)) = self.missing.remove(&sid)
                && !self.reports(&identity, source)
            {
                changes.push(Change {
                    identity,
                    source,
                    missing: false,
                });
            }
        }
        changes
    }

    fn overdue<'a>(&mut self, now: Instant, wanted: &'a [Wanted]) -> Vec<&'a Wanted> {
        let mut overdue = Vec::new();
        for track in wanted {
            if self.missing.contains_key(&track.sid) {
                continue;
            }
            let since = *self.waiting.entry(track.sid.clone()).or_insert(now);
            if now.duration_since(since) >= RECEIVE_WAIT {
                overdue.push(track);
            }
        }
        overdue.sort_by(|a, b| a.sid.cmp(&b.sid));
        overdue
    }

    fn reports(&self, identity: &MediaIdentity, source: MediaSource) -> bool {
        self.missing
            .values()
            .any(|(reported, reported_source)| reported == identity && *reported_source == source)
    }
}
