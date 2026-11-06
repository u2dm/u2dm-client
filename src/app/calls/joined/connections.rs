use std::collections::HashMap;

use tokio::time::Instant;

use super::{Joined, Link, LinkState, sent_videos};
use crate::app::calls::media::MediaCommand;
use crate::commands::view::{LinkView, ServerDetails};
use crate::domain::call::MemberKey;
use crate::domain::call::sfu::{SfuId, SfuRole};
use crate::domain::call::stats::{ConnectionReading, Rates, ReceivedVideo, rates};
use crate::domain::call::video::{FrameSize, VideoKind};
use crate::domain::member_name::NamedUser;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Meter {
    Off,
    Waiting,
    Level(u8),
}

pub(super) type Readings = HashMap<SfuId, (Instant, ConnectionReading, Rates)>;

impl Joined {
    pub(in crate::app::calls) fn watch_microphone(&mut self, on: bool) {
        if (self.microphone_meter != Meter::Off) == on {
            return;
        }
        tracing::debug!(call = self.call, on, "watching your microphone's activity");
        self.microphone_meter = if on { Meter::Waiting } else { Meter::Off };
        self.command(MediaCommand::WatchMicrophone(on));
    }

    pub(in crate::app::calls) fn watch_connections(&mut self, on: bool) {
        if self.readings.is_some() == on {
            return;
        }
        tracing::info!(
            call = self.call,
            on,
            "watching the call's connection details"
        );
        self.readings = on.then(HashMap::new);
        self.command(MediaCommand::WatchConnections(on));
    }

    pub(super) fn connections_read(&mut self, readings: Vec<ConnectionReading>) {
        let Some(kept) = self.readings.as_mut() else {
            return;
        };
        let now = Instant::now();
        for reading in readings {
            let rates = kept
                .get(&reading.sfu)
                .map(|(at, previous, _)| {
                    let elapsed = now.saturating_duration_since(*at);
                    rates(
                        previous,
                        &reading,
                        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                    )
                })
                .unwrap_or_default();
            kept.insert(reading.sfu.clone(), (now, reading, rates));
        }
    }

    pub(super) fn connection_details(&self, readings: &Readings) -> Vec<ServerDetails> {
        let mut links: Vec<(&SfuId, &Link)> = self.links.iter().collect();
        links.sort_by_cached_key(|(sfu, link)| (link.role != SfuRole::Publish, sfu.host()));
        links
            .into_iter()
            .map(|(sfu, link)| {
                let reading = readings.get(sfu);
                let rates = reading.map(|(_, _, rates)| *rates).unwrap_or_default();
                ServerDetails {
                    host: sfu.host(),
                    publishing: link.role == SfuRole::Publish,
                    state: match link.state {
                        LinkState::Asking | LinkState::Connecting => LinkView::Connecting,
                        LinkState::Connected => LinkView::Connected,
                        LinkState::Reconnecting => LinkView::Reconnecting,
                        LinkState::Failed(_) => LinkView::Failed,
                    },
                    sent_kbps: rates.sent_kbps,
                    received_kbps: rates.received_kbps,
                    loss_permille: rates.loss_permille,
                    round_trip_ms: reading.and_then(|(_, reading, _)| reading.round_trip_ms),
                    sent_video: reading
                        .map(|(_, reading, _)| sent_videos(&reading.sent_video))
                        .unwrap_or_default(),
                    received_video: reading
                        .map(|(_, reading, _)| self.named_videos(&reading.received_video))
                        .unwrap_or_default(),
                }
            })
            .collect()
    }

    fn named_videos(&self, videos: &[ReceivedVideo]) -> Vec<(NamedUser, VideoKind, FrameSize)> {
        videos
            .iter()
            .filter_map(|video| {
                let key = self.identities.member_of(&video.identity)?;
                let member = self
                    .members
                    .iter()
                    .find(|member| MemberKey::of(member) == *key)?;
                Some((member.user.clone(), video.kind, video.size))
            })
            .collect()
    }
}
