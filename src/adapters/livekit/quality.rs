use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::media::ConnectionQuality;

const QUALITY_EVALUATION: Duration = Duration::from_secs(5);
pub(super) const POOR_HOLD: Duration = QUALITY_EVALUATION.saturating_mul(2);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum QualityOf {
    You,
    Member(MediaIdentity),
}

pub(super) type Rated = (QualityOf, ConnectionQuality);

#[derive(Default)]
pub(super) struct Qualities {
    shown: HashMap<QualityOf, ConnectionQuality>,
    poor_since: HashMap<QualityOf, Instant>,
}

impl Qualities {
    pub(super) fn rated(
        &mut self,
        of: QualityOf,
        quality: ConnectionQuality,
        now: Instant,
    ) -> Option<Rated> {
        let shown = self.shown_for(&of);
        if quality != ConnectionQuality::Poor {
            self.poor_since.remove(&of);
        }
        match (quality, shown) {
            (ConnectionQuality::Good, ConnectionQuality::Good)
            | (ConnectionQuality::Lost, ConnectionQuality::Lost)
            | (ConnectionQuality::Poor, ConnectionQuality::Poor) => None,
            (ConnectionQuality::Poor, ConnectionQuality::Good) => {
                self.poor_since.entry(of).or_insert(now);
                None
            }
            (ConnectionQuality::Good, _) => {
                self.shown.remove(&of);
                Some((of, ConnectionQuality::Good))
            }
            (ConnectionQuality::Lost | ConnectionQuality::Poor, _) => {
                self.shown.insert(of.clone(), quality);
                Some((of, quality))
            }
        }
    }

    pub(super) fn due(&mut self, now: Instant) -> Vec<Rated> {
        let due: Vec<QualityOf> = self
            .poor_since
            .iter()
            .filter(|(_, since)| now.duration_since(**since) >= POOR_HOLD)
            .map(|(of, _)| of.clone())
            .collect();
        for of in &due {
            self.poor_since.remove(of);
            self.shown.insert(of.clone(), ConnectionQuality::Poor);
        }
        due.into_iter()
            .map(|of| (of, ConnectionQuality::Poor))
            .collect()
    }

    pub(super) fn next_due(&self) -> Option<Instant> {
        self.poor_since
            .values()
            .min()
            .map(|since| *since + POOR_HOLD)
    }

    pub(super) fn forget(&mut self, of: &QualityOf) -> Option<Rated> {
        self.poor_since.remove(of);
        self.shown
            .remove(of)
            .map(|_| (of.clone(), ConnectionQuality::Good))
    }

    fn shown_for(&self, of: &QualityOf) -> ConnectionQuality {
        self.shown
            .get(of)
            .copied()
            .unwrap_or(ConnectionQuality::Good)
    }
}
