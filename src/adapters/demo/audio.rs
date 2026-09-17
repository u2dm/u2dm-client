use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};

const ENV_VAR: &str = "U2DM_DEMO_AUDIO";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "makes audio playback deterministic and its download observable",
    combinable: true,
    flags: &[
        Flag {
            value: "silent",
            effect: "plays audio on the wall clock without opening a sound device",
            note: "use it for every scripted run: a headless launch still reaches PipeWire, and real output makes position depend on the host",
        },
        Flag {
            value: "slow",
            effect: "each audio download waits ~1.5s, so the bubble's loading state is reachable",
            note: "",
        },
        Flag {
            value: "all",
            effect: "silent and slow",
            note: "",
        },
    ],
    notes: &["`-missing-download` style ids still fail the download the way thumbnails do"],
};
const DOWNLOAD_DELAY: Duration = Duration::from_millis(1500);

#[derive(Default, Clone)]
pub struct Scenario {
    pub silent: bool,
    pub download_is_slow: bool,
}

pub fn scenario() -> &'static Scenario {
    static SCENARIO: OnceLock<Scenario> = OnceLock::new();
    SCENARIO.get_or_init(from_env)
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
        silent = scenario.silent,
        download_is_slow = scenario.download_is_slow,
        "demo mode: shaping audio playback"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "silent" => scenario.silent = true,
        "slow" => scenario.download_is_slow = true,
        "all" => {
            scenario.silent = true;
            scenario.download_is_slow = true;
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub async fn pause_download() {
    if scenario().download_is_slow {
        sleep(DOWNLOAD_DELAY).await;
    }
}
