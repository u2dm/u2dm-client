use std::collections::HashSet;
use std::env;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};
use super::data;
use crate::domain::identity::{OwnIdentity, RoomAlert};
use crate::domain::member_name::NamedUser;
use crate::domain::room::RoomId;
use crate::domain::user_info::IdentityTrust;

const ENV_VAR: &str = "U2DM_DEMO_IDENTITY";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "raises identity alerts: reset identities and lost verifications that hold your sends",
    combinable: true,
    flags: &[
        Flag {
            value: "reset",
            effect: "Jordan and Nadia reset their identities, so Design Team (and weekend-trip, for Nadia) shows them one at a time",
            note: "OK accepts the shown one and the next takes its place",
        },
        Flag {
            value: "verified-reset",
            effect: "Sarah, whom you verified, is no longer verified: every encrypted room she is in holds your texts and polls, and her sheet says so",
            note: "Withdraw verification lifts the hold; a held message then needs Retry",
        },
        Flag {
            value: "own-reset",
            effect: "your own identity was reset on another device",
            note: "the banner shows in every room until acknowledged",
        },
        Flag {
            value: "slow",
            effect: "a room's alerts arrive 2s after it opens and your own 3s after login, as a keys query brings them, and every banner action takes 1.5s",
            note: "the banner's button shows its busy state meanwhile",
        },
        Flag {
            value: "acks-fail",
            effect: "accepting, withdrawing and acknowledging all fail",
            note: "each failure is a toast, and the alert stays",
        },
        Flag {
            value: "all",
            effect: "reset, verified-reset and own-reset",
            note: "excludes slow and acks-fail",
        },
    ],
    notes: &[
        "only encrypted rooms raise room alerts, as only their messages are encrypted",
        "stickers and attachments are never held in demo mode",
    ],
};

const RESET_MEMBERS: [&str; 2] = ["@jordan:matrix.org", "@nadia:matrix.org"];
const VERIFIED_MEMBER: &str = "@sarah:matrix.org";
const ROOM_ALERT_DELAY: Duration = Duration::from_secs(2);
const OWN_ALERT_DELAY: Duration = Duration::from_secs(3);
const ACTION_DELAY: Duration = Duration::from_millis(1500);

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    resets: bool,
    verified_reset: bool,
    own_reset: bool,
    slow: bool,
    pub acks_fail: bool,
}

pub fn scenario() -> Scenario {
    static SCENARIO: OnceLock<Scenario> = OnceLock::new();
    *SCENARIO.get_or_init(from_env)
}

fn from_env() -> Scenario {
    let Ok(raw) = env::var(ENV_VAR) else {
        return Scenario::default();
    };
    let mut scenario = Scenario::default();
    for flag in raw.split(',').map(str::trim) {
        apply(&mut scenario, flag);
    }
    tracing::info!(
        resets = scenario.resets,
        verified_reset = scenario.verified_reset,
        own_reset = scenario.own_reset,
        slow = scenario.slow,
        acks_fail = scenario.acks_fail,
        "demo mode: raising identity alerts"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "reset" => scenario.resets = true,
        "verified-reset" => scenario.verified_reset = true,
        "own-reset" => scenario.own_reset = true,
        "slow" => scenario.slow = true,
        "acks-fail" => scenario.acks_fail = true,
        "all" => {
            scenario.resets = true;
            scenario.verified_reset = true;
            scenario.own_reset = true;
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub async fn pause_room_alerts() {
    if scenario().slow {
        sleep(ROOM_ALERT_DELAY).await;
    }
}

pub async fn pause_own_identity() {
    if scenario().slow {
        sleep(OWN_ALERT_DELAY).await;
    }
}

pub async fn pause_action() {
    if scenario().slow {
        sleep(ACTION_DELAY).await;
    }
}

#[derive(Default)]
struct Decisions {
    accepted: HashSet<String>,
    withdrawn: HashSet<String>,
    own_acknowledged: bool,
}

impl Decisions {
    fn no_longer_verified(&self, room_id: &RoomId) -> bool {
        scenario().verified_reset
            && !self.withdrawn.contains(VERIFIED_MEMBER)
            && data::is_member(room_id, VERIFIED_MEMBER)
    }
}

pub enum SendHold {
    Clear,
    VerificationLost,
}

pub struct IdentityBook {
    decisions: Mutex<Decisions>,
    revision: watch::Sender<u64>,
}

impl Default for IdentityBook {
    fn default() -> Self {
        Self {
            decisions: Mutex::default(),
            revision: watch::Sender::new(0),
        }
    }
}

impl IdentityBook {
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }

    fn decide<T>(&self, decide: impl FnOnce(&mut Decisions) -> T) -> Option<T> {
        let decided = self
            .decisions
            .lock()
            .ok()
            .map(|mut decisions| decide(&mut decisions));
        self.revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
        decided
    }

    fn read<T>(&self, read: impl FnOnce(&Decisions) -> T) -> Option<T> {
        self.decisions.lock().ok().map(|decisions| read(&decisions))
    }

    pub fn room_alerts(&self, room_id: &RoomId) -> Vec<RoomAlert> {
        if !data::room_is_encrypted(room_id) {
            return Vec::new();
        }
        let book = data::names_in(room_id, []);
        let named = |user_id: &str| NamedUser::new(user_id, book.of(user_id));
        self.read(|decisions| {
            let mut alerts = Vec::new();
            if scenario().resets {
                alerts.extend(
                    RESET_MEMBERS
                        .iter()
                        .filter(|user_id| {
                            !decisions.accepted.contains(**user_id)
                                && data::is_member(room_id, user_id)
                        })
                        .map(|user_id| RoomAlert::IdentityReset(named(user_id))),
                );
            }
            if decisions.no_longer_verified(room_id) {
                alerts.push(RoomAlert::NoLongerVerified(named(VERIFIED_MEMBER)));
            }
            alerts
        })
        .unwrap_or_default()
    }

    pub fn own_identity(&self) -> OwnIdentity {
        let reset = scenario().own_reset
            && !self
                .read(|decisions| decisions.own_acknowledged)
                .unwrap_or_default();
        if reset {
            OwnIdentity::Reset
        } else {
            OwnIdentity::Unchanged
        }
    }

    pub fn send_hold(&self, room_id: &RoomId) -> SendHold {
        if !data::room_is_encrypted(room_id) {
            return SendHold::Clear;
        }
        self.read(|decisions| {
            if decisions.no_longer_verified(room_id) {
                SendHold::VerificationLost
            } else {
                SendHold::Clear
            }
        })
        .unwrap_or(SendHold::Clear)
    }

    pub fn trust_of(&self, user_id: &str) -> Option<IdentityTrust> {
        if !scenario().verified_reset || user_id != VERIFIED_MEMBER {
            return None;
        }
        let withdrawn = self
            .read(|decisions| decisions.withdrawn.contains(user_id))
            .unwrap_or_default();
        Some(if withdrawn {
            IdentityTrust::Unverified
        } else {
            IdentityTrust::NoLongerVerified
        })
    }

    pub fn accept(&self, user_id: &str) -> Result<(), String> {
        if !RESET_MEMBERS.contains(&user_id) {
            return Err(format!("{user_id} did not reset their identity"));
        }
        self.decide(|decisions| decisions.accepted.insert(user_id.to_owned()));
        Ok(())
    }

    pub fn withdraw(&self, user_id: &str) -> Result<(), String> {
        if user_id != VERIFIED_MEMBER {
            return Err(format!("{user_id} was never verified"));
        }
        self.decide(|decisions| decisions.withdrawn.insert(user_id.to_owned()));
        Ok(())
    }

    pub fn acknowledge_own_reset(&self) {
        self.decide(|decisions| decisions.own_acknowledged = true);
    }
}
