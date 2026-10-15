use std::collections::HashSet;
use std::env;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};

const ENV_VAR: &str = "U2DM_DEMO_LINKS";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "shapes room previews and joins from Matrix links and the Join a room field",
    combinable: true,
    flags: &[
        Flag {
            value: "slow",
            effect: "each preview, join and preview avatar waits ~1.5s, so the dialog shows Looking up and Joining",
            note: "",
        },
        Flag {
            value: "preview-fails",
            effect: "each address's first preview fails, so Retry is reachable",
            note: "",
        },
        Flag {
            value: "unavailable",
            effect: "a room you are not in cannot be previewed, though Join still works",
            note: "an alias nothing names still has no room",
        },
        Flag {
            value: "join-fails",
            effect: "every join fails, inline while the dialog shows the room and as a toast after",
            note: "",
        },
        Flag {
            value: "no-echo",
            effect: "a joined room never lists, so the dialog stays on Joining until it closes",
            note: "",
        },
    ],
    notes: &[
        "#rust:demo.local and #matrix:demo.local name joined rooms; #matrix-dev:demo.local and #rust-wasm:demo.local name unjoined ones",
        "!matrix-core:demo.local is invite-only, !matrix-design:demo.local takes knocks, !rust-archive:demo.local is private, !space-rust-gamedev:demo.local is an unjoined space",
        "a join reaches the room list ~0.8s after it returns, as a real sync echo does",
    ],
};
const SLOW_DELAY: Duration = Duration::from_millis(1500);

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub is_slow: bool,
    pub preview_fails: bool,
    pub unavailable: bool,
    pub join_fails: bool,
    pub echo_lost: bool,
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
        preview_fails = scenario.preview_fails,
        unavailable = scenario.unavailable,
        join_fails = scenario.join_fails,
        echo_lost = scenario.echo_lost,
        "demo mode: shaping room previews from links"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" => scenario.is_slow = true,
        "preview-fails" => scenario.preview_fails = true,
        "unavailable" => scenario.unavailable = true,
        "join-fails" => scenario.join_fails = true,
        "no-echo" => scenario.echo_lost = true,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub async fn pause() {
    if scenario().is_slow {
        sleep(SLOW_DELAY).await;
    }
}

fn failed_previews() -> &'static Mutex<HashSet<String>> {
    static FAILED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    FAILED.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn preview_fails_now(address: &str) -> bool {
    if !scenario().preview_fails {
        return false;
    }
    failed_previews()
        .lock()
        .is_ok_and(|mut failed| failed.insert(address.to_owned()))
}
