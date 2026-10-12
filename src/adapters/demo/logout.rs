use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};
use crate::ports::matrix::DeviceRemoval;

const ENV_VAR: &str = "U2DM_DEMO_LOGOUT";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "shapes how the homeserver answers a logout: Log Out, an undone login and a diverged restore",
    combinable: true,
    flags: &[
        Flag {
            value: "slow",
            effect: "every logout waits ~3s for the homeserver's answer",
            note: "the only way to see the signing-out status, or to quit during it",
        },
        Flag {
            value: "fails",
            effect: "the homeserver never confirms a logout, so the left-behind notice shows",
            note: "",
        },
    ],
    notes: &[
        "unset, the homeserver confirms every logout at once",
        "Sign out instead never asks the homeserver, so it shows the notice without this variable",
    ],
};
const SLOW_DELAY: Duration = Duration::from_secs(3);

#[derive(Default, Clone, Copy)]
struct Scenario {
    is_slow: bool,
    never_confirms: bool,
}

fn scenario() -> Scenario {
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
        never_confirms = scenario.never_confirms,
        "demo mode: shaping how the homeserver answers a logout"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" => scenario.is_slow = true,
        "fails" => scenario.never_confirms = true,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub async fn answer() -> DeviceRemoval {
    let scenario = scenario();
    if scenario.is_slow {
        sleep(SLOW_DELAY).await;
    }
    if scenario.never_confirms {
        tracing::info!("demo mode: the homeserver did not confirm the logout");
        DeviceRemoval::Unconfirmed
    } else {
        tracing::info!("demo mode: the homeserver signed this device out");
        DeviceRemoval::Confirmed
    }
}
