use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::watch;
use tokio::time::sleep;

use super::catalog::{Flag, Scenarios, scenario_from_env};
use crate::adapters::settings::JsonSettings;
use crate::domain::server_rules::{RoomDefault, ServerRules};
use crate::domain::settings::Settings;
use crate::error::{AppError, Result};
use crate::ports::settings::SettingsPort;

const ENV_VAR: &str = "U2DM_DEMO_SETTINGS";
const SHAPING: &str = "demo mode: shaping the settings store";
const FILE_PREFIX: &str = "file=";
const SLOW_MEDIA_LOAD: Duration = Duration::from_millis(1500);
const SLOW_CACHE: Duration = Duration::from_millis(1500);
const SLOW_RULE_WRITE: Duration = Duration::from_millis(1500);
const RULE_ECHO_LAG: Duration = Duration::from_millis(600);
const STARTING_KEYWORDS: [&str; 2] = ["standup", "deploy"];

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
        Flag {
            value: "rules-slow",
            effect: "each change to a notification rule takes 1.5 s, so its pending row shows",
            note: "",
        },
        Flag {
            value: "rules-fail",
            effect: "every change to a notification rule fails, so the change is taken back and a toast shows",
            note: "",
        },
        Flag {
            value: "rules-no-echo",
            effect: "a change to a notification rule succeeds but never comes back through sync, so the target stays shown and a new keyword stays pending",
            note: "",
        },
        Flag {
            value: "keywords",
            effect: "the account starts with two keywords",
            note: "",
        },
        Flag {
            value: "account-muted",
            effect: "the account starts with Notifications for this account off",
            note: "",
        },
        Flag {
            value: "rules-missing",
            effect: "the homeserver has no account-wide rule, so its switch is dimmed and inert",
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
    rules_are_slow: bool,
    rules_fail: bool,
    rule_echoes_are_lost: bool,
    starts_with_keywords: bool,
    account_is_muted: bool,
    account_rule_is_missing: bool,
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

pub async fn pause_rule_write() {
    if scenario().rules_are_slow {
        sleep(SLOW_RULE_WRITE).await;
    }
}

pub fn rule_writes_fail() -> bool {
    scenario().rules_fail
}

fn starting_rules() -> ServerRules {
    let demo = scenario();
    ServerRules {
        account_notifications: (!demo.account_rule_is_missing).then_some(!demo.account_is_muted),
        direct: RoomDefault::AllMessages,
        group: RoomDefault::AllMessages,
        keywords: if demo.starts_with_keywords {
            STARTING_KEYWORDS.map(str::to_owned).to_vec()
        } else {
            Vec::new()
        },
    }
}

pub struct RulesBook(watch::Sender<ServerRules>);

impl Default for RulesBook {
    fn default() -> Self {
        Self(watch::Sender::new(starting_rules()))
    }
}

impl RulesBook {
    pub fn subscribe(&self) -> watch::Receiver<ServerRules> {
        self.0.subscribe()
    }

    pub fn echo(&self, change: impl FnOnce(&mut ServerRules) + Send + 'static) {
        if scenario().rule_echoes_are_lost {
            return;
        }
        let book = self.0.clone();
        tokio::spawn(async move {
            sleep(RULE_ECHO_LAG).await;
            book.send_modify(change);
        });
    }
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
        "rules-slow" => scenario.rules_are_slow = true,
        "rules-fail" => scenario.rules_fail = true,
        "rules-no-echo" => scenario.rule_echoes_are_lost = true,
        "keywords" => scenario.starts_with_keywords = true,
        "account-muted" => scenario.account_is_muted = true,
        "rules-missing" => scenario.account_rule_is_missing = true,
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
