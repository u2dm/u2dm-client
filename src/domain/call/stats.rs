use super::identity::MediaIdentity;
use super::sfu::SfuId;
use super::video::{FrameSize, VideoKind};

const BITS_PER_BYTE: u64 = 8;
const PERMILLE: u64 = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
pub struct ConnectionReading {
    pub sfu: SfuId,
    pub sent_bytes: Option<u64>,
    pub received_bytes: Option<u64>,
    pub received_packets: Option<u64>,
    pub lost_packets: Option<u64>,
    pub round_trip_ms: Option<u32>,
    pub sent_video: Vec<SentVideo>,
    pub received_video: Vec<ReceivedVideo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SentVideo {
    pub kind: VideoKind,
    pub size: FrameSize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedVideo {
    pub identity: MediaIdentity,
    pub kind: VideoKind,
    pub size: FrameSize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rates {
    pub sent_kbps: Option<u32>,
    pub received_kbps: Option<u32>,
    pub loss_permille: Option<u16>,
}

pub fn rates(previous: &ConnectionReading, now: &ConnectionReading, elapsed_ms: u64) -> Rates {
    if elapsed_ms == 0 {
        return Rates::default();
    }
    let kbps = |before: Option<u64>, after: Option<u64>| {
        let bits = after?.checked_sub(before?)?.saturating_mul(BITS_PER_BYTE);
        u32::try_from(bits / elapsed_ms).ok()
    };
    let received = now
        .received_packets
        .zip(previous.received_packets)
        .and_then(|(after, before)| after.checked_sub(before));
    let lost = now
        .lost_packets
        .zip(previous.lost_packets)
        .and_then(|(after, before)| after.checked_sub(before));
    let loss_permille = received.zip(lost).and_then(|(received, lost)| {
        let expected = received.saturating_add(lost);
        (expected > 0)
            .then(|| u16::try_from(lost.saturating_mul(PERMILLE) / expected).ok())
            .flatten()
    });
    Rates {
        sent_kbps: kbps(previous.sent_bytes, now.sent_bytes),
        received_kbps: kbps(previous.received_bytes, now.received_bytes),
        loss_permille,
    }
}
