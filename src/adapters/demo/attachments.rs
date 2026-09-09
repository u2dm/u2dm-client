use super::catalog::{Flag, Scenarios};
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::time::sleep;

const ENV_VAR: &str = "U2DM_DEMO_ATTACHMENTS";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "reproduces attachment upload timing and refusals",
    combinable: true,
    flags: &[
        Flag {
            value: "slow",
            effect: "the echo lands as Uploading and ticks to 100% over ~5s",
            note: "",
        },
        Flag {
            value: "send-fails",
            effect: "the in-dialog error",
            note: "excluded from `all`",
        },
        Flag {
            value: "too-large",
            effect: "the server size refusal, unreachable without a real server",
            note: "excluded from `all`",
        },
        Flag {
            value: "pick=<path>",
            effect: "skips the native file dialog and picks that file",
            note: "REQUIRED for any scripted attachment run: a native chooser cannot be driven by the MCP inspector, and it blocks the Slint event loop",
        },
        Flag {
            value: "all",
            effect: "slow only",
            note: "",
        },
    ],
    notes: &[],
};
const UPLOAD_DELAY: Duration = Duration::from_millis(2500);
pub const UPLOAD_STEPS: u64 = 12;
pub const UPLOAD_STEP_DELAY: Duration = Duration::from_millis(400);
const PRETEND_UPLOAD_LIMIT: u64 = 1024;
const PICK_PREFIX: &str = "pick=";

#[derive(Default, Clone)]
pub struct Scenario {
    pub upload_is_slow: bool,
    pub send_fails: bool,
    pub refuses_size: bool,
    pub preset_pick: Option<PathBuf>,
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
        upload_is_slow = scenario.upload_is_slow,
        send_fails = scenario.send_fails,
        refuses_size = scenario.refuses_size,
        preset_pick = ?scenario.preset_pick,
        "demo mode: reproducing real-account attachment upload timing"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" | "all" => scenario.upload_is_slow = true,
        "send-fails" => scenario.send_fails = true,
        "too-large" => scenario.refuses_size = true,
        preset if preset.starts_with(PICK_PREFIX) => {
            scenario.preset_pick = preset.strip_prefix(PICK_PREFIX).map(PathBuf::from);
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub async fn pause_upload() {
    if scenario().upload_is_slow {
        sleep(UPLOAD_DELAY).await;
    }
}

pub fn upload_limit() -> u64 {
    PRETEND_UPLOAD_LIMIT
}

fn previews() -> &'static Mutex<HashMap<String, PathBuf>> {
    static PREVIEWS: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    PREVIEWS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn remember_preview(event_id: &str, path: &Path) {
    if let Ok(mut guard) = previews().lock() {
        guard.insert(event_id.to_owned(), path.to_path_buf());
    }
}

pub fn preview_path(event_id: &str) -> Option<PathBuf> {
    previews().lock().ok()?.get(event_id).cloned()
}
