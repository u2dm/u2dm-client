use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::sleep;

use super::catalog::{Flag, Scenarios, scenario_from_env};

const ENV_VAR: &str = "U2DM_DEMO_PINNED";
const SHAPING: &str = "demo mode: shaping pinned messages";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "reproduces how a room's pinned messages load and change",
    combinable: true,
    flags: &[
        Flag {
            value: "slow",
            effect: "the pinned messages arrive ~1.5s after the room opens, the way a first fetch of them does",
            note: "the bar then appears over a timeline that has already settled",
        },
        Flag {
            value: "repin",
            effect: "the room's newest message is pinned, then unpinned, every 2s",
            note: "the bar follows the newest pin unless it was stepped back to an older one",
        },
        Flag {
            value: "all",
            effect: "slow and repin",
            note: "",
        },
    ],
    notes: &["a room pins the message ids listed under its `pinned` in the fixture"],
};
const ARRIVAL_DELAY: Duration = Duration::from_millis(1500);
pub const REPIN_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Default, Clone)]
pub struct Scenario {
    pub arrives_late: bool,
    pub repins: bool,
}

pub fn scenario() -> &'static Scenario {
    static SCENARIO: OnceLock<Scenario> = OnceLock::new();
    SCENARIO.get_or_init(|| scenario_from_env(ENV_VAR, apply, SHAPING))
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" => scenario.arrives_late = true,
        "repin" => scenario.repins = true,
        "all" => {
            scenario.arrives_late = true;
            scenario.repins = true;
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub async fn pause_arrival() {
    if scenario().arrives_late {
        sleep(ARRIVAL_DELAY).await;
    }
}
