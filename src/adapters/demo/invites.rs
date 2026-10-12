use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};

const ENV_VAR: &str = "U2DM_DEMO_INVITES";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "shapes invites: when they arrive or go, and how accepting and declining answer",
    combinable: true,
    flags: &[
        Flag {
            value: "slow",
            effect: "accepting and declining wait ~1.5s, so the sheet's busy buttons are reachable",
            note: "",
        },
        Flag {
            value: "accept-fails",
            effect: "every accept fails, so the inline error and the toast are reachable",
            note: "the toast shows when the sheet was closed before the answer",
        },
        Flag {
            value: "decline-fails",
            effect: "every decline fails",
            note: "",
        },
        Flag {
            value: "no-echo",
            effect: "an accepted room never lists, so the sheet stays on Joining until closed",
            note: "",
        },
        Flag {
            value: "late",
            effect: "Leo's chat invite arrives ~3s after sync starts, lighting the DMs and All tiles live",
            note: "the stock fixture's !invite-leo:demo.local",
        },
        Flag {
            value: "revoked",
            effect: "the Climbing Crew invite is withdrawn ~4s after sync starts, closing its open sheet",
            note: "the stock fixture's !invite-climbing:demo.local",
        },
    ],
    notes: &[
        "an answer reaches the room list ~0.8s after it returns, as a real sync echo does",
        "Async Rust is a child of the Rust space, so the Rust tile lights too",
    ],
};
pub const LATE_INVITE: &str = "!invite-leo:demo.local";
pub const LATE_ARRIVAL: Duration = Duration::from_secs(3);
pub const REVOKED_INVITE: &str = "!invite-climbing:demo.local";
pub const REVOKED_AFTER: Duration = Duration::from_secs(4);
const SLOW_DELAY: Duration = Duration::from_millis(1500);

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub is_slow: bool,
    pub accept_fails: bool,
    pub decline_fails: bool,
    pub echo_lost: bool,
    pub arrives_late: bool,
    pub gets_revoked: bool,
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
        accept_fails = scenario.accept_fails,
        decline_fails = scenario.decline_fails,
        echo_lost = scenario.echo_lost,
        arrives_late = scenario.arrives_late,
        gets_revoked = scenario.gets_revoked,
        "demo mode: shaping invites"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" => scenario.is_slow = true,
        "accept-fails" => scenario.accept_fails = true,
        "decline-fails" => scenario.decline_fails = true,
        "no-echo" => scenario.echo_lost = true,
        "late" => scenario.arrives_late = true,
        "revoked" => scenario.gets_revoked = true,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub async fn pause() {
    if scenario().is_slow {
        sleep(SLOW_DELAY).await;
    }
}
