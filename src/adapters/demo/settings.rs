use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::sleep;

use super::catalog::{Flag, Scenarios, scenario_from_env};
use crate::adapters::settings::JsonSettings;
use crate::domain::settings::Settings;
use crate::error::{AppError, Result};
use crate::ports::settings::SettingsPort;

const ENV_VAR: &str = "U2DM_DEMO_SETTINGS";
const SHAPING: &str = "demo mode: shaping the settings store";
const FILE_PREFIX: &str = "file=";
const SLOW_MEDIA_LOAD: Duration = Duration::from_millis(1500);
const SLOW_CACHE: Duration = Duration::from_millis(1500);

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "shapes where the settings live and how saving them goes",
    combinable: true,
    flags: &[
        Flag {
            value: "file=<path>",
            effect: "keeps the settings in that JSON file, through the production reader and writer",
            note: "REQUIRED to prove a relaunch keeps them: unset, they live in memory and start at the defaults",
        },
        Flag {
            value: "save-fails",
            effect: "every save fails, so a change raises the not-saved toast",
            note: "the change still applies for this run",
        },
        Flag {
            value: "media-slow",
            effect: "media you ask to load takes 1.5 s, so its spinner shows",
            note: "",
        },
        Flag {
            value: "cache-slow",
            effect: "measuring and clearing downloaded media take 1.5 s, so Measuring and the busy button show",
            note: "",
        },
        Flag {
            value: "clear-fails",
            effect: "clearing downloaded media fails, so the not-cleared toast shows and the size is measured again",
            note: "",
        },
    ],
    notes: &[
        "unset, the settings start at the defaults and are not kept: a demo run never reads your real file",
    ],
};

#[derive(Clone, Default)]
#[allow(clippy::struct_excessive_bools)]
struct Scenario {
    file: Option<PathBuf>,
    save_fails: bool,
    media_is_slow: bool,
    cache_is_slow: bool,
    clearing_fails: bool,
}

pub async fn pause_media_load() {
    if scenario().media_is_slow {
        sleep(SLOW_MEDIA_LOAD).await;
    }
}

pub async fn pause_cache() {
    if scenario().cache_is_slow {
        sleep(SLOW_CACHE).await;
    }
}

pub fn clearing_fails() -> bool {
    scenario().clearing_fails
}

fn scenario() -> Scenario {
    static SCENARIO: OnceLock<Scenario> = OnceLock::new();
    SCENARIO
        .get_or_init(|| scenario_from_env(ENV_VAR, apply, SHAPING))
        .clone()
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "save-fails" => scenario.save_fails = true,
        "media-slow" => scenario.media_is_slow = true,
        "cache-slow" => scenario.cache_is_slow = true,
        "clear-fails" => scenario.clearing_fails = true,
        preset if preset.starts_with(FILE_PREFIX) => {
            scenario.file = preset.strip_prefix(FILE_PREFIX).map(PathBuf::from);
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub fn port() -> Arc<dyn SettingsPort> {
    let Scenario {
        file, save_fails, ..
    } = scenario();
    Arc::new(DemoSettings {
        file: file.map(JsonSettings::new),
        save_fails,
    })
}

struct DemoSettings {
    file: Option<JsonSettings>,
    save_fails: bool,
}

#[async_trait]
impl SettingsPort for DemoSettings {
    fn load(&self) -> Settings {
        self.file
            .as_ref()
            .map_or_else(Settings::default, SettingsPort::load)
    }

    async fn save(&self, settings: &Settings) -> Result<()> {
        if self.save_fails {
            return Err(AppError::Other(
                "demo: saving the settings fails".to_owned(),
            ));
        }
        tracing::info!(?settings, "demo: the settings are saved");
        match &self.file {
            Some(file) => file.save(settings).await,
            None => Ok(()),
        }
    }
}
