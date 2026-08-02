use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::sleep;

const ENV_VAR: &str = "U2DM_DEMO_STICKERS";
const CATALOG_DELAY: Duration = Duration::from_millis(900);
const TRICKLE_DELAY: Duration = Duration::from_millis(300);

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub catalog_is_slow: bool,
    pub images_trickle: bool,
    pub catalog_is_empty: bool,
    pub catalog_fails: bool,
    pub send_fails: bool,
    pub room_is_encrypted: bool,
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
        catalog_is_slow = scenario.catalog_is_slow,
        images_trickle = scenario.images_trickle,
        catalog_is_empty = scenario.catalog_is_empty,
        catalog_fails = scenario.catalog_fails,
        send_fails = scenario.send_fails,
        room_is_encrypted = scenario.room_is_encrypted,
        "demo mode: reproducing real-account sticker pack timing"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" => scenario.catalog_is_slow = true,
        "trickle" => scenario.images_trickle = true,
        "empty" => scenario.catalog_is_empty = true,
        "fails" => scenario.catalog_fails = true,
        "send-fails" => scenario.send_fails = true,
        "encrypted" => scenario.room_is_encrypted = true,
        "all" => {
            scenario.catalog_is_slow = true;
            scenario.images_trickle = true;
            scenario.room_is_encrypted = true;
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub async fn pause_catalog() {
    if scenario().catalog_is_slow {
        sleep(CATALOG_DELAY).await;
    }
}

pub async fn pause_prefetch() {
    if scenario().images_trickle {
        sleep(TRICKLE_DELAY).await;
    }
}
