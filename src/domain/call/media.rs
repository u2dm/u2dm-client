use std::fmt;

use super::devices::{DeviceChoices, DeviceKey, DeviceKind, DeviceLists};
use super::identity::MediaIdentity;
use super::sfu::SfuId;
use super::stats::ConnectionReading;
use super::video::{Camera, FeedId, ScreenShare, VideoKind};
use crate::domain::room::RoomId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSetup {
    pub room_id: RoomId,
    pub encrypted: bool,
    pub devices: DeviceChoices,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaChoice {
    pub microphone: bool,
    pub camera: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
pub enum SfuState {
    Connecting,
    Connected,
    Reconnecting,
    Disconnected(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
pub enum Decryption {
    Ok,
    MissingKey,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
pub enum EncryptionTrouble {
    MissingKey,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
pub enum MediaSource {
    Microphone,
    Camera,
    Screen,
    ScreenAudio,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
pub enum ConnectionQuality {
    Good,
    Poor,
    Lost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Microphone {
    #[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
    Off,
    On,
    #[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
    Failed(String),
}

impl fmt::Display for Microphone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Off => f.write_str("off"),
            Self::On => f.write_str("on"),
            Self::Failed(why) => write!(f, "failed: {why}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(any(feature = "calls", feature = "demo")), allow(dead_code))]
pub enum MediaEvent {
    Sfu {
        sfu: SfuId,
        state: SfuState,
    },
    Present {
        sfu: SfuId,
        identity: MediaIdentity,
        present: bool,
    },
    Muted {
        sfu: SfuId,
        identity: MediaIdentity,
        muted: bool,
    },
    Speaking {
        sfu: SfuId,
        speakers: Vec<MediaIdentity>,
    },
    Missing {
        sfu: SfuId,
        identity: MediaIdentity,
        source: MediaSource,
        missing: bool,
    },
    Quality {
        sfu: SfuId,
        identity: MediaIdentity,
        quality: ConnectionQuality,
    },
    Stalled {
        sfu: SfuId,
        identity: MediaIdentity,
        stalled: bool,
    },
    OwnQuality {
        sfu: SfuId,
        quality: ConnectionQuality,
    },
    Decryption {
        identity: MediaIdentity,
        state: Decryption,
    },
    NotEncrypted {
        sfu: SfuId,
        trouble: EncryptionTrouble,
    },
    Microphone(Microphone),
    MicrophoneLevel(u8),
    Connections(Vec<ConnectionReading>),
    Camera(Camera),
    Screen(ScreenShare),
    Video {
        sfu: SfuId,
        identity: MediaIdentity,
        kind: VideoKind,
        feed: Option<FeedId>,
    },
    Devices(DeviceLists),
    DeviceFailed {
        kind: DeviceKind,
        device: DeviceKey,
        why: String,
    },
}
