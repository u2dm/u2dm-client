use crate::app::calls::device_test::TestOutcome;
use crate::app::calls::lane::LaneEvent;
use crate::app::calls::preview::PreviewOutcome;
use crate::domain::call::devices::DeviceLists;
use crate::domain::call::hands::HandSighting;
use crate::domain::call::media::MediaEvent;
use crate::domain::call::reactions::ReactionOption;
use crate::domain::call::ring::{CallSignal, RingId};
use crate::domain::call::sfu::{GrantResult, SfuId};
use crate::domain::call::{DeviceStanding, KeySendReport, MemberKey, TransportLookup};
use crate::domain::room::RoomId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum RingDeadline {
    Unconfirmed,
    Expired,
}

pub(in crate::app) enum CallEvent {
    Signal {
        session: u64,
        signal: CallSignal,
    },
    RingWindow {
        ring: RingId,
        deadline: RingDeadline,
    },
    Declined {
        room_id: RoomId,
        sent: bool,
    },
    Transports {
        session: u64,
        found: Option<TransportLookup>,
    },
    Lane {
        call: u64,
        event: LaneEvent,
    },
    Media {
        call: u64,
        event: MediaEvent,
    },
    MediaFailed {
        call: u64,
        why: String,
    },
    Granted {
        call: u64,
        sfu: SfuId,
        outcome: Box<GrantResult>,
    },
    Announced {
        call: u64,
        ring: Option<RingId>,
    },
    RingOutTimeout {
        call: u64,
    },
    KeyWake {
        call: u64,
    },
    StageWake {
        call: u64,
    },
    ReactionWake {
        call: u64,
    },
    FacesReady {
        session: u64,
        ready: usize,
    },
    KeysSent {
        call: u64,
        report: KeySendReport,
    },
    Standings {
        call: u64,
        standings: Vec<(MemberKey, DeviceStanding)>,
    },
    HandSent {
        call: u64,
        membership_event: String,
        outcome: Result<String, String>,
    },
    HandLowered {
        call: u64,
        reaction: String,
        ok: bool,
    },
    HandsFetched {
        call: u64,
        sightings: Vec<HandSighting>,
    },
    ReactionSent {
        call: u64,
        membership_event: String,
        reaction: ReactionOption,
        outcome: Result<String, String>,
    },
    DevicesListed {
        listing: u64,
        lists: DeviceLists,
    },
    Preview {
        preview: u64,
        outcome: PreviewOutcome,
    },
    DeviceTest {
        test: u64,
        outcome: TestOutcome,
    },
}

impl CallEvent {
    pub(in crate::app) const fn call(&self) -> Option<u64> {
        match self {
            Self::Lane { call, .. }
            | Self::Media { call, .. }
            | Self::MediaFailed { call, .. }
            | Self::Granted { call, .. }
            | Self::Announced { call, .. }
            | Self::RingOutTimeout { call }
            | Self::KeyWake { call }
            | Self::StageWake { call }
            | Self::ReactionWake { call }
            | Self::KeysSent { call, .. }
            | Self::Standings { call, .. }
            | Self::HandSent { call, .. }
            | Self::HandLowered { call, .. }
            | Self::HandsFetched { call, .. }
            | Self::ReactionSent { call, .. } => Some(*call),
            Self::Signal { .. }
            | Self::RingWindow { .. }
            | Self::Declined { .. }
            | Self::Transports { .. }
            | Self::FacesReady { .. }
            | Self::DevicesListed { .. }
            | Self::Preview { .. }
            | Self::DeviceTest { .. } => None,
        }
    }

    pub(in crate::app) fn label(&self) -> &'static str {
        match self {
            Self::Signal { .. } => "CallSignal",
            Self::RingWindow { .. } => "CallRingWindow",
            Self::Declined { .. } => "CallDeclined",
            Self::Transports { .. } => "CallTransports",
            Self::Lane { .. } => "CallLane",
            Self::Media { .. } => "CallMedia",
            Self::MediaFailed { .. } => "CallMediaFailed",
            Self::Granted { .. } => "CallGranted",
            Self::Announced { .. } => "CallAnnounced",
            Self::RingOutTimeout { .. } => "CallRingOutTimeout",
            Self::KeyWake { .. } => "CallKeyWake",
            Self::StageWake { .. } => "CallStageWake",
            Self::ReactionWake { .. } => "CallReactionWake",
            Self::FacesReady { .. } => "CallFacesReady",
            Self::KeysSent { .. } => "CallKeysSent",
            Self::Standings { .. } => "CallStandings",
            Self::HandSent { .. } => "CallHandSent",
            Self::HandLowered { .. } => "CallHandLowered",
            Self::HandsFetched { .. } => "CallHandsFetched",
            Self::ReactionSent { .. } => "CallReactionSent",
            Self::DevicesListed { .. } => "CallDevicesListed",
            Self::Preview { .. } => "CallPreview",
            Self::DeviceTest { .. } => "CallDeviceTest",
        }
    }
}
