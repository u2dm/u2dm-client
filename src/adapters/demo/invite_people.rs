use std::collections::HashSet;
use std::env;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};

const ENV_VAR: &str = "U2DM_DEMO_INVITE_PEOPLE";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "shapes the user directory search and the invites sent from the Invite people dialog",
    combinable: true,
    flags: &[
        Flag {
            value: "slow",
            effect: "each search and each invite waits ~1.5s, so the dialog shows Searching and Inviting",
            note: "",
        },
        Flag {
            value: "search-fails",
            effect: "each search term's first search fails, so Retry is reachable",
            note: "",
        },
        Flag {
            value: "rate-limited",
            effect: "every search is rate-limited",
            note: "",
        },
        Flag {
            value: "limited",
            effect: "a search returns at most 3 people and says more match",
            note: "",
        },
        Flag {
            value: "invite-fails",
            effect: "every invite fails, inline while the dialog is open and as a toast after",
            note: "",
        },
        Flag {
            value: "blocked",
            effect: "inviting @vera:matrix.org fails because she blocks invites, the rest succeed",
            note: "",
        },
        Flag {
            value: "no-permission",
            effect: "no room or space lets you invite, so every Invite people entry point hides",
            note: "",
        },
    ],
    notes: &[
        "the directory holds everyone who speaks in a fixture timeline; a full user ID nobody lists is offered anyway",
        "an invited person shows under Invited in room info ~0.6s after the invite returns",
    ],
};
const SLOW_DELAY: Duration = Duration::from_millis(1500);
pub const LIMITED_RESULTS: usize = 3;
pub const BLOCKED_USER: &str = "@vera:matrix.org";

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub is_slow: bool,
    pub search_fails_once: bool,
    pub rate_limited: bool,
    pub limited: bool,
    pub invite_fails: bool,
    pub blocks_vera: bool,
    pub no_permission: bool,
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
        search_fails_once = scenario.search_fails_once,
        rate_limited = scenario.rate_limited,
        limited = scenario.limited,
        invite_fails = scenario.invite_fails,
        blocks_vera = scenario.blocks_vera,
        no_permission = scenario.no_permission,
        "demo mode: shaping the people search and invites"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" => scenario.is_slow = true,
        "search-fails" => scenario.search_fails_once = true,
        "rate-limited" => scenario.rate_limited = true,
        "limited" => scenario.limited = true,
        "invite-fails" => scenario.invite_fails = true,
        "blocked" => scenario.blocks_vera = true,
        "no-permission" => scenario.no_permission = true,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub async fn pause() {
    if scenario().is_slow {
        sleep(SLOW_DELAY).await;
    }
}

pub fn may_invite() -> bool {
    !scenario().no_permission
}

fn failed_searches() -> &'static Mutex<HashSet<String>> {
    static FAILED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    FAILED.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn search_fails_now(term: &str) -> bool {
    if !scenario().search_fails_once {
        return false;
    }
    failed_searches()
        .lock()
        .is_ok_and(|mut failed| failed.insert(term.to_owned()))
}
