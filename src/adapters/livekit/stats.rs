use std::collections::HashMap;

use livekit::prelude::{LocalTrack, RemoteTrack, Room, TrackSource};
use livekit::webrtc::stats::RtcStats;

use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::sfu::SfuId;
use crate::domain::call::stats::{ConnectionReading, ReceivedVideo, SentVideo};
use crate::domain::call::video::{FrameSize, VideoKind};

const VIDEO: &str = "video";
const MILLIS_PER_SECOND: f64 = 1000.0;

pub(super) async fn reading(sfu: &SfuId, room: &Room) -> Option<ConnectionReading> {
    let stats = match room.get_stats().await {
        Ok(stats) => stats,
        Err(e) => {
            tracing::debug!(%sfu, "the call connection's statistics could not be read: {e}");
            return None;
        }
    };
    let mut reading = ConnectionReading {
        sfu: sfu.clone(),
        sent_bytes: None,
        received_bytes: None,
        received_packets: None,
        lost_packets: None,
        round_trip_ms: None,
        sent_video: Vec::new(),
        received_video: Vec::new(),
    };
    let all = || stats.publisher_stats.iter().chain(&stats.subscriber_stats);
    let videos = Videos {
        own: own_video_tracks(room),
        sources: all()
            .filter_map(|stats| match stats {
                RtcStats::MediaSource(source) => Some((
                    source.rtc.id.as_str(),
                    source.source.track_identifier.as_str(),
                )),
                _ => None,
            })
            .collect(),
        remote: video_tracks(room),
    };
    for stats in all() {
        read(&mut reading, stats, &videos);
    }
    Some(reading)
}

struct Videos<'a> {
    own: HashMap<String, VideoKind>,
    sources: HashMap<&'a str, &'a str>,
    remote: HashMap<String, (MediaIdentity, VideoKind)>,
}

impl Videos<'_> {
    fn sent_kind(&self, media_source_id: &str) -> Option<VideoKind> {
        let track = self.sources.get(media_source_id)?;
        self.own.get(*track).copied()
    }
}

fn read(reading: &mut ConnectionReading, stats: &RtcStats, videos: &Videos<'_>) {
    match stats {
        RtcStats::OutboundRtp(outbound) => {
            add(&mut reading.sent_bytes, outbound.sent.bytes_sent);
            let size = FrameSize::new(
                outbound.outbound.frame_width,
                outbound.outbound.frame_height,
            );
            if outbound.stream.kind == VIDEO
                && size.pixels() > 0
                && let Some(kind) = videos.sent_kind(&outbound.outbound.media_source_id)
            {
                keep_largest(&mut reading.sent_video, kind, size);
            }
        }
        RtcStats::InboundRtp(inbound) => {
            add(&mut reading.received_bytes, inbound.inbound.bytes_received);
            add(
                &mut reading.received_packets,
                inbound.received.packets_received,
            );
            add(
                &mut reading.lost_packets,
                u64::try_from(inbound.received.packets_lost).unwrap_or(0),
            );
            let size = FrameSize::new(inbound.inbound.frame_width, inbound.inbound.frame_height);
            if inbound.stream.kind == VIDEO
                && size.pixels() > 0
                && let Some((identity, kind)) = videos.remote.get(&inbound.inbound.track_identifier)
            {
                reading.received_video.push(ReceivedVideo {
                    identity: identity.clone(),
                    kind: *kind,
                    size,
                });
            }
        }
        RtcStats::CandidatePair(pair) => round_trip(
            reading,
            pair.candidate_pair.nominated,
            pair.candidate_pair.current_round_trip_time,
        ),
        _ => {}
    }
}

fn keep_largest(sent: &mut Vec<SentVideo>, kind: VideoKind, size: FrameSize) {
    match sent.iter_mut().find(|video| video.kind == kind) {
        Some(video) if size.pixels() > video.size.pixels() => video.size = size,
        Some(_) => {}
        None => sent.push(SentVideo { kind, size }),
    }
}

fn add(total: &mut Option<u64>, value: u64) {
    *total = Some(total.unwrap_or(0).saturating_add(value));
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn round_trip(reading: &mut ConnectionReading, nominated: bool, seconds: f64) {
    if reading.round_trip_ms.is_none() && nominated && seconds > 0.0 {
        reading.round_trip_ms = Some((seconds * MILLIS_PER_SECOND).round() as u32);
    }
}

fn video_kind(source: TrackSource) -> Option<VideoKind> {
    match source {
        TrackSource::Camera => Some(VideoKind::Camera),
        TrackSource::Screenshare => Some(VideoKind::Screen),
        TrackSource::Unknown | TrackSource::Microphone | TrackSource::ScreenshareAudio => None,
    }
}

fn own_video_tracks(room: &Room) -> HashMap<String, VideoKind> {
    room.local_participant()
        .track_publications()
        .into_values()
        .filter_map(|publication| {
            let kind = video_kind(publication.source())?;
            match publication.track()? {
                LocalTrack::Video(track) => Some((track.rtc_track().id(), kind)),
                LocalTrack::Audio(_) => None,
            }
        })
        .collect()
}

fn video_tracks(room: &Room) -> HashMap<String, (MediaIdentity, VideoKind)> {
    let mut tracks = HashMap::new();
    for participant in room.remote_participants().into_values() {
        let identity = MediaIdentity::new(participant.identity().to_string());
        for publication in participant.track_publications().into_values() {
            let Some(kind) = video_kind(publication.source()) else {
                continue;
            };
            if let Some(RemoteTrack::Video(track)) = publication.track() {
                tracks.insert(track.rtc_track().id(), (identity.clone(), kind));
            }
        }
    }
    tracks
}
