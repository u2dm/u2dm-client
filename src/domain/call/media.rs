use std::fmt;

use super::identity::MediaIdentity;
use super::sfu::SfuId;
use crate::domain::room::RoomId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSetup {
    pub room_id: RoomId,
    pub encrypted: bool,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Microphone {
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
    Decryption {
        identity: MediaIdentity,
        state: Decryption,
    },
    Microphone(Microphone),
}
