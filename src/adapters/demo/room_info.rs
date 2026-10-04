use std::collections::HashSet;
use std::env;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};
use crate::domain::room::RoomId;

const ENV_VAR: &str = "U2DM_DEMO_ROOM_INFO";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "shapes the room info sheet and the room menu: the member list, notification changes, leaving, marking read and room links",
    combinable: true,
    flags: &[
        Flag {
            value: "slow",
            effect: "the member list, each notification change, leave, mark-as-read and room link wait ~1.5s, and a member's avatar exists only once its fetch batch lands",
            note: "the loading, busy and leaving states are only reachable with `slow`",
        },
        Flag {
            value: "roster-fails",
            effect: "the first member list of each room fails once, so the retry notice is reachable",
            note: "",
        },
        Flag {
            value: "notify-fails",
            effect: "every notification change fails, so the inline error is reachable",
            note: "",
        },
        Flag {
            value: "leave-fails",
            effect: "every leave fails, so the inline error is reachable",
            note: "",
        },
        Flag {
            value: "read-fails",
            effect: "every mark-as-read fails, so its toast is reachable",
            note: "",
        },
        Flag {
            value: "link-fails",
            effect: "every room link fails, so its toast is reachable",
            note: "",
        },
        Flag {
            value: "no-echo",
            effect: "a notification change succeeds but never reaches the room list, so the pending state stays up",
            note: "",
        },
    ],
    notes: &[
        "members are you, the room's timeline senders and role holders, then Guest fillers up to the room's member count, then the invited",
        "a notification change, a leave or a mark-as-read returns first and reaches the room list ~0.6s later, as a real sync echo does",
    ],
};
pub const ECHO_LAG: Duration = Duration::from_millis(600);
const SLOW_DELAY: Duration = Duration::from_millis(1500);
const AVATAR_TRICKLE: Duration = Duration::from_millis(400);
const MEMBER_AVATAR_PREFIX: &str = "member/";

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub is_slow: bool,
    pub roster_fails_once: bool,
    pub notify_fails: bool,
    pub leave_fails: bool,
    pub read_fails: bool,
    pub link_fails: bool,
    pub echo_is_lost: bool,
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
        is_slow = scenario.is_slow,
        roster_fails_once = scenario.roster_fails_once,
        notify_fails = scenario.notify_fails,
        leave_fails = scenario.leave_fails,
        read_fails = scenario.read_fails,
        link_fails = scenario.link_fails,
        echo_is_lost = scenario.echo_is_lost,
        "demo mode: shaping the room info sheet and the room menu"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" => scenario.is_slow = true,
        "roster-fails" => scenario.roster_fails_once = true,
        "notify-fails" => scenario.notify_fails = true,
        "leave-fails" => scenario.leave_fails = true,
        "read-fails" => scenario.read_fails = true,
        "link-fails" => scenario.link_fails = true,
        "no-echo" => scenario.echo_is_lost = true,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub async fn pause() {
    if scenario().is_slow {
        sleep(SLOW_DELAY).await;
    }
}

pub async fn pause_avatars() {
    if scenario().is_slow {
        sleep(AVATAR_TRICKLE).await;
    }
}

fn failed_rosters() -> &'static Mutex<HashSet<RoomId>> {
    static FAILED: OnceLock<Mutex<HashSet<RoomId>>> = OnceLock::new();
    FAILED.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn roster_fails_now(room_id: &RoomId) -> bool {
    scenario().roster_fails_once
        && failed_rosters()
            .lock()
            .is_ok_and(|mut failed| failed.insert(room_id.clone()))
}

pub fn member_avatar(user_id: &str) -> String {
    if scenario().is_slow {
        format!("{MEMBER_AVATAR_PREFIX}{user_id}")
    } else {
        user_id.to_owned()
    }
}

fn fetched_member_avatars() -> &'static Mutex<HashSet<String>> {
    static FETCHED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    FETCHED.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn remember_fetched_avatars(mxcs: &[String]) {
    if let Ok(mut fetched) = fetched_member_avatars().lock() {
        fetched.extend(mxcs.iter().cloned());
    }
}

pub fn avatar_owner(mxc: &str) -> Option<&str> {
    let Some(user_id) = mxc.strip_prefix(MEMBER_AVATAR_PREFIX) else {
        return Some(mxc);
    };
    fetched_member_avatars()
        .lock()
        .is_ok_and(|fetched| fetched.contains(mxc))
        .then_some(user_id)
}
