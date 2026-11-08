use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

use tokio::time::sleep;

use super::catalog::{Flag, Scenarios, scenario_from_env};

const ENV_VAR: &str = "U2DM_DEMO_ATTACHMENTS";
const SHAPING: &str = "demo mode: reproducing real-account attachment upload timing";

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
            note: "REQUIRED for any scripted dialog pick: a native chooser cannot be driven by the MCP inspector, and it blocks the Slint event loop; a Ctrl+V paste ignores it and reads the real clipboard",
        },
        Flag {
            value: "save-to=<dir>",
            effect: "skips the native save dialog and saves into that existing directory, under the name the dialog would suggest",
            note: "REQUIRED for any scripted save (Save as…, a file's save button, the probe's `save_file`): the native dialog cannot be driven by the MCP inspector",
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
pub const PRETEND_UPLOAD_LIMIT: u64 = 1024;
const PICK_PREFIX: &str = "pick=";
const SAVE_PREFIX: &str = "save-to=";

#[derive(Default, Clone)]
pub struct Scenario {
    pub upload_is_slow: bool,
    pub send_fails: bool,
    pub refuses_size: bool,
    pub preset_pick: Option<PathBuf>,
    pub save_dir: Option<PathBuf>,
}

pub fn scenario() -> &'static Scenario {
    static SCENARIO: OnceLock<Scenario> = OnceLock::new();
    SCENARIO.get_or_init(|| scenario_from_env(ENV_VAR, apply, SHAPING))
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" | "all" => scenario.upload_is_slow = true,
        "send-fails" => scenario.send_fails = true,
        "too-large" => scenario.refuses_size = true,
        preset if preset.starts_with(PICK_PREFIX) => {
            scenario.preset_pick = preset.strip_prefix(PICK_PREFIX).map(PathBuf::from);
        }
        preset if preset.starts_with(SAVE_PREFIX) => {
            scenario.save_dir = preset.strip_prefix(SAVE_PREFIX).map(PathBuf::from);
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub async fn pause_upload() {
    if scenario().upload_is_slow {
        sleep(UPLOAD_DELAY).await;
    }
}

static PREVIEWS: LazyLock<Mutex<HashMap<String, PathBuf>>> = LazyLock::new(Mutex::default);

pub fn remember_preview(event_id: &str, path: &Path) {
    if let Ok(mut guard) = PREVIEWS.lock() {
        guard.insert(event_id.to_owned(), path.to_path_buf());
    }
}

pub fn preview_path(event_id: &str) -> Option<PathBuf> {
    PREVIEWS.lock().ok()?.get(event_id).cloned()
}

static SENT_FILES: LazyLock<Mutex<HashMap<String, PathBuf>>> = LazyLock::new(Mutex::default);

pub fn remember_sent_file(event_id: &str, path: &Path) {
    if let Ok(mut guard) = SENT_FILES.lock() {
        guard.insert(event_id.to_owned(), path.to_path_buf());
    }
}

pub fn sent_file_path(event_id: &str) -> Option<PathBuf> {
    SENT_FILES.lock().ok()?.get(event_id).cloned()
}
