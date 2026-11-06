use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::call_book::{CallBook, Joiner, sfu_id};
use super::{JORDAN, NADIA, scenario};
use crate::domain::call::media::{ConnectionQuality, MediaEvent, MediaSource};
use crate::domain::call::sfu::{SfuId, SfuRole};

const TROUBLE_AFTER: Duration = Duration::from_secs(3);

const UNRECEIVED_FOR: Duration = Duration::from_secs(8);

const POOR_FOR: Duration = Duration::from_secs(8);

const LOST_AFTER: Duration = Duration::from_secs(7);

const LOST_FOR: Duration = Duration::from_secs(5);

const STALLED_FOR: Duration = Duration::from_secs(8);

impl CallBook {
    pub fn troubles(&self, room_id: &str, sfu: &str, role: SfuRole) -> Vec<Trouble> {
        let Some(id) = sfu_id(sfu) else {
            return Vec::new();
        };
        let here = self.joiners(room_id, |joiner| {
            joiner.sfu == sfu && joiner.on_main_device()
        });
        let mut troubles = Vec::new();
        for joiner in here {
            troubles.extend(missing_media(&id, joiner));
            troubles.extend(lost_connection(&id, joiner));
            troubles.extend(stalled_video(&id, joiner));
        }
        if scenario().poor_connection && role == SfuRole::Publish {
            troubles.push(Trouble::own(
                &id,
                TROUBLE_AFTER,
                ConnectionQuality::Poor,
                "demo: your connection is poor",
            ));
            troubles.push(Trouble::own(
                &id,
                TROUBLE_AFTER + POOR_FOR,
                ConnectionQuality::Good,
                "demo: your connection is good again",
            ));
        }
        troubles.sort_by_key(|trouble| trouble.after);
        troubles
    }
}

pub struct Trouble {
    pub after: Duration,
    pub event: MediaEvent,
    pub says: &'static str,
}

impl Trouble {
    fn missing(
        sfu: &SfuId,
        joiner: Joiner,
        after: Duration,
        source: MediaSource,
        missing: bool,
        says: &'static str,
    ) -> Self {
        Self {
            after,
            event: MediaEvent::Missing {
                sfu: sfu.clone(),
                identity: joiner.identity(),
                source,
                missing,
            },
            says,
        }
    }

    fn rated(
        sfu: &SfuId,
        joiner: Joiner,
        after: Duration,
        quality: ConnectionQuality,
        says: &'static str,
    ) -> Self {
        Self {
            after,
            event: MediaEvent::Quality {
                sfu: sfu.clone(),
                identity: joiner.identity(),
                quality,
            },
            says,
        }
    }

    fn own(sfu: &SfuId, after: Duration, quality: ConnectionQuality, says: &'static str) -> Self {
        Self {
            after,
            event: MediaEvent::OwnQuality {
                sfu: sfu.clone(),
                quality,
            },
            says,
        }
    }
}

fn missing_media(sfu: &SfuId, joiner: Joiner) -> Vec<Trouble> {
    let shaped = scenario();
    let mut troubles = Vec::new();
    let jordan_audio_missing = || {
        Trouble::missing(
            sfu,
            joiner,
            TROUBLE_AFTER,
            MediaSource::Microphone,
            true,
            "demo: Jordan's audio is not received",
        )
    };
    if joiner.user == JORDAN && shaped.unreceived_audio {
        troubles.push(jordan_audio_missing());
        troubles.push(Trouble::missing(
            sfu,
            joiner,
            TROUBLE_AFTER + UNRECEIVED_FOR,
            MediaSource::Microphone,
            false,
            "demo: Jordan's audio arrives",
        ));
    }
    if joiner.user == JORDAN && shaped.unreceived_media {
        troubles.push(jordan_audio_missing());
        troubles.push(Trouble::missing(
            sfu,
            joiner,
            TROUBLE_AFTER,
            MediaSource::Camera,
            true,
            "demo: Jordan's camera is not received",
        ));
    }
    if joiner.user == NADIA && shaped.unreceived_media {
        troubles.push(Trouble::missing(
            sfu,
            joiner,
            TROUBLE_AFTER,
            MediaSource::Microphone,
            true,
            "demo: Nadia's audio is not received",
        ));
    }
    troubles
}

fn stalled_video(sfu: &SfuId, joiner: Joiner) -> Vec<Trouble> {
    if !(scenario().video_stalls && joiner.user == JORDAN) {
        return Vec::new();
    }
    let stall = |after: Duration, stalled: bool, says: &'static str| Trouble {
        after,
        event: MediaEvent::Stalled {
            sfu: sfu.clone(),
            identity: joiner.identity(),
            stalled,
        },
        says,
    };
    vec![
        stall(TROUBLE_AFTER, true, "demo: Jordan's camera stops arriving"),
        stall(
            TROUBLE_AFTER + STALLED_FOR,
            false,
            "demo: Jordan's camera flows again",
        ),
    ]
}

fn lost_connection(sfu: &SfuId, joiner: Joiner) -> Vec<Trouble> {
    if !(scenario().connection_lost && joiner.user == JORDAN) {
        return Vec::new();
    }
    vec![
        Trouble::rated(
            sfu,
            joiner,
            TROUBLE_AFTER,
            ConnectionQuality::Poor,
            "demo: Jordan's connection is poor",
        ),
        Trouble::rated(
            sfu,
            joiner,
            LOST_AFTER,
            ConnectionQuality::Lost,
            "demo: Jordan's connection is lost",
        ),
        Trouble::rated(
            sfu,
            joiner,
            LOST_AFTER + LOST_FOR,
            ConnectionQuality::Good,
            "demo: Jordan's connection is good again",
        ),
    ]
}

pub fn announcement_fails() -> bool {
    static FAILED: AtomicBool = AtomicBool::new(false);
    scenario().announce_fails && !FAILED.swap(true, Ordering::AcqRel)
}

pub fn cuts_off() -> bool {
    static CUT: AtomicBool = AtomicBool::new(false);
    scenario().cut_off && !CUT.swap(true, Ordering::AcqRel)
}

pub fn fails_to_encrypt() -> bool {
    static FAILED: AtomicBool = AtomicBool::new(false);
    scenario().not_encrypted && !FAILED.swap(true, Ordering::AcqRel)
}

pub fn loses_camera() -> bool {
    static LOST: AtomicBool = AtomicBool::new(false);
    scenario().camera_lost && !LOST.swap(true, Ordering::AcqRel)
}
