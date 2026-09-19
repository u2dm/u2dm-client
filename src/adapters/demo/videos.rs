use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};

const ENV_VAR: &str = "U2DM_DEMO_VIDEO";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "makes a video's download long enough to act on while it runs",
    combinable: true,
    flags: &[Flag {
        value: "slow",
        effect: "each video download waits ~3s, so the player can be closed or replaced mid-download",
        note: "",
    }],
    notes: &["the player's loading state is only reachable with `slow`"],
};
const DOWNLOAD_DELAY: Duration = Duration::from_secs(3);

#[derive(Default, Clone)]
pub struct Scenario {
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
        download_is_slow = scenario.download_is_slow,
        "demo mode: shaping video downloads"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" => scenario.download_is_slow = true,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub async fn pause_download() {
    if scenario().download_is_slow {
        sleep(DOWNLOAD_DELAY).await;
    }
}
