use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};
use super::invite_people;
use crate::domain::room::SpacePowers;

const ENV_VAR: &str = "U2DM_DEMO_CREATE_ROOM";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "shapes creating rooms and spaces from the New room and New space dialog",
    combinable: true,
    flags: &[
        Flag {
            value: "slow",
            effect: "each creation waits ~1.5s, so the dialog shows Creating",
            note: "",
        },
        Flag {
            value: "fails",
            effect: "every creation fails, inline while the dialog is open and as a toast after",
            note: "",
        },
        Flag {
            value: "refused",
            effect: "the homeserver refuses every creation with a 403",
            note: "",
        },
        Flag {
            value: "child-fails",
            effect: "the room is created but adding it to the space fails, so a toast says so",
            note: "the room still opens, at the top level",
        },
        Flag {
            value: "parent-late",
            effect: "the parent space lists a new child ~2s after the room lists, so the dialog waits",
            note: "",
        },
        Flag {
            value: "no-echo",
            effect: "a created room never lists, so the dialog stays on Creating until it closes",
            note: "",
        },
        Flag {
            value: "no-space-rights",
            effect: "no space lets you add rooms, so the dialog never offers one",
            note: "",
        },
    ],
    notes: &[
        "an address is taken when a fixture room or an earlier creation already uses its localpart, for example #rust or #matrix",
        "a created room lists ~0.8s after the creation returns, as a real sync echo does",
    ],
};
const SLOW_DELAY: Duration = Duration::from_millis(1500);
pub const PARENT_LAG: Duration = Duration::from_secs(2);

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub is_slow: bool,
    pub fails: bool,
    pub refused: bool,
    pub child_fails: bool,
    pub parent_late: bool,
    pub echo_lost: bool,
    pub no_space_rights: bool,
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
        fails = scenario.fails,
        refused = scenario.refused,
        child_fails = scenario.child_fails,
        parent_late = scenario.parent_late,
        echo_lost = scenario.echo_lost,
        no_space_rights = scenario.no_space_rights,
        "demo mode: shaping room creation"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" => scenario.is_slow = true,
        "fails" => scenario.fails = true,
        "refused" => scenario.refused = true,
        "child-fails" => scenario.child_fails = true,
        "parent-late" => scenario.parent_late = true,
        "no-echo" => scenario.echo_lost = true,
        "no-space-rights" => scenario.no_space_rights = true,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub async fn pause() {
    if scenario().is_slow {
        sleep(SLOW_DELAY).await;
    }
}

pub fn space_powers() -> SpacePowers {
    SpacePowers {
        invite: invite_people::may_invite(),
        add_children: !scenario().no_space_rights,
    }
}
