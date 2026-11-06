use std::collections::{HashMap, HashSet};

use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::media::{ConnectionQuality, Decryption, MediaSource};
use crate::domain::call::sfu::SfuId;
use crate::domain::call::video::{FeedId, VideoKind};

#[derive(Default)]
pub(super) struct Heard {
    pub(super) present: HashMap<SfuId, HashSet<MediaIdentity>>,
    pub(super) decryption: HashMap<MediaIdentity, Decryption>,
    pub(super) muted: HashMap<SfuId, HashSet<MediaIdentity>>,
    pub(super) speaking: HashMap<SfuId, Vec<MediaIdentity>>,
    video: HashMap<SfuId, HashMap<MediaIdentity, FeedId>>,
    screens: HashMap<SfuId, HashMap<MediaIdentity, FeedId>>,
    missing: HashMap<SfuId, HashMap<MediaIdentity, HashSet<MediaSource>>>,
    quality: HashMap<SfuId, HashMap<MediaIdentity, ConnectionQuality>>,
    stalled: HashMap<SfuId, HashSet<MediaIdentity>>,
}

impl Heard {
    pub(super) fn is_present(&self, identity: &MediaIdentity) -> bool {
        self.present
            .values()
            .any(|present| present.contains(identity))
    }

    pub(super) fn is_speaking(&self, identity: &MediaIdentity) -> bool {
        self.speaking
            .values()
            .any(|speaking| speaking.contains(identity))
    }

    pub(super) fn is_muted_on(&self, sfu: &SfuId, identity: &MediaIdentity) -> bool {
        self.muted
            .get(sfu)
            .is_some_and(|muted| muted.contains(identity))
    }

    pub(super) fn video_on(
        &self,
        sfu: &SfuId,
        identities: &[MediaIdentity],
        kind: VideoKind,
    ) -> Option<FeedId> {
        let feeds = match kind {
            VideoKind::Camera => &self.video,
            VideoKind::Screen => &self.screens,
        }
        .get(sfu)?;
        identities
            .iter()
            .find_map(|identity| feeds.get(identity))
            .cloned()
    }

    pub(super) fn video_heard(
        &mut self,
        sfu: SfuId,
        identity: MediaIdentity,
        kind: VideoKind,
        feed: Option<FeedId>,
    ) {
        if kind == VideoKind::Camera
            && feed.is_none()
            && let Some(stalled) = self.stalled.get_mut(&sfu)
        {
            stalled.remove(&identity);
        }
        let feeds = match kind {
            VideoKind::Camera => &mut self.video,
            VideoKind::Screen => &mut self.screens,
        }
        .entry(sfu)
        .or_default();
        match feed {
            Some(feed) => feeds.insert(identity, feed),
            None => feeds.remove(&identity),
        };
    }

    pub(super) fn source_heard(
        &mut self,
        sfu: SfuId,
        identity: MediaIdentity,
        source: MediaSource,
        missing: bool,
    ) {
        let sources = self
            .missing
            .entry(sfu)
            .or_default()
            .entry(identity)
            .or_default();
        if missing {
            sources.insert(source);
        } else {
            sources.remove(&source);
        }
    }

    pub(super) fn missing_on(
        &self,
        sfu: &SfuId,
        identities: &[MediaIdentity],
    ) -> HashSet<MediaSource> {
        self.missing
            .get(sfu)
            .into_iter()
            .flat_map(|missing| {
                identities
                    .iter()
                    .filter_map(|identity| missing.get(identity))
            })
            .flatten()
            .copied()
            .collect()
    }

    pub(super) fn rated(
        &mut self,
        sfu: SfuId,
        identity: MediaIdentity,
        quality: ConnectionQuality,
    ) {
        let ratings = self.quality.entry(sfu).or_default();
        if quality == ConnectionQuality::Good {
            ratings.remove(&identity);
        } else {
            ratings.insert(identity, quality);
        }
    }

    pub(super) fn quality_on(
        &self,
        sfu: &SfuId,
        identities: &[MediaIdentity],
    ) -> ConnectionQuality {
        let ratings = self.quality.get(sfu);
        let rated_as = |quality| {
            identities
                .iter()
                .any(|identity| ratings.and_then(|ratings| ratings.get(identity)) == Some(&quality))
        };
        if rated_as(ConnectionQuality::Lost) {
            ConnectionQuality::Lost
        } else if rated_as(ConnectionQuality::Poor) {
            ConnectionQuality::Poor
        } else {
            ConnectionQuality::Good
        }
    }

    pub(super) fn stalled_heard(&mut self, sfu: SfuId, identity: MediaIdentity, stalled: bool) {
        let identities = self.stalled.entry(sfu).or_default();
        if stalled {
            identities.insert(identity);
        } else {
            identities.remove(&identity);
        }
    }

    pub(super) fn is_stalled_on(&self, sfu: &SfuId, identities: &[MediaIdentity]) -> bool {
        self.stalled
            .get(sfu)
            .is_some_and(|stalled| identities.iter().any(|identity| stalled.contains(identity)))
    }

    pub(super) fn forget(&mut self, sfu: &SfuId) {
        self.stalled.remove(sfu);
        self.present.remove(sfu);
        self.muted.remove(sfu);
        self.speaking.remove(sfu);
        self.video.remove(sfu);
        self.screens.remove(sfu);
        self.missing.remove(sfu);
        self.quality.remove(sfu);
    }
}
