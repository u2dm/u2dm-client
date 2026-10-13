use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};

const ENV_VAR: &str = "U2DM_DEMO_UPGRADES";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "shapes joins across room upgrades, from the banner and from the creation row",
    combinable: true,
    flags: &[
        Flag {
            value: "slow",
            effect: "each join waits ~1.5s, so the banner and the creation row show Joining",
            note: "",
        },
        Flag {
            value: "join-fails",
            effect: "every join fails with its toast",
            note: "",
        },
        Flag {
            value: "no-echo",
            effect: "a joined room never lists: its link stays on Joining until another room opens",
            note: "",
        },
    ],
    notes: &[
        "Retro Gaming (!retro:demo.local) was upgraded to an unjoined room: its banner joins it",
        "Frontend Guild continues the unjoined !frontend-old:demo.local: its creation row joins it",
        "a join reaches the room list ~0.8s after it returns, as a real sync echo does",
    ],
};
const SLOW_DELAY: Duration = Duration::from_millis(1500);

#[derive(Default, Clone, Copy)]
pub struct Scenario {
    pub is_slow: bool,
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
        join_fails = scenario.join_fails,
        echo_lost = scenario.echo_lost,
        "demo mode: shaping joins across room upgrades"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" => scenario.is_slow = true,
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
