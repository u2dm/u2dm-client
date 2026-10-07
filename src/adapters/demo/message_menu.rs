use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};
use crate::domain::message::MessagePermissions;

const ENV_VAR: &str = "U2DM_DEMO_MESSAGE_MENU";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "shapes what the right-click message menu's actions answer",
    combinable: true,
    flags: &[
        Flag {
            value: "slow",
            effect: "copying a message link, each pin or unpin and each deletion wait ~1.5s",
            note: "",
        },
        Flag {
            value: "link-fails",
            effect: "every message link fails, so its toast is reachable",
            note: "",
        },
        Flag {
            value: "source-unavailable",
            effect: "View source finds no event, so the dialog's unavailable state shows",
            note: "",
        },
        Flag {
            value: "delete-refused",
            effect: "every deletion is refused before it is queued, so the message stays and the toast shows",
            note: "",
        },
        Flag {
            value: "delete-fails",
            effect: "a deleted message comes back a second later with the toast, the way a refused queued redaction is discarded",
            note: "",
        },
        Flag {
            value: "pin-fails",
            effect: "every pin and unpin is refused, so their toasts are reachable",
            note: "",
        },
        Flag {
            value: "member",
            effect: "every room's power levels let you delete only your own messages and not pin",
            note: "",
        },
        Flag {
            value: "barred",
            effect: "every room's power levels deny deleting and pinning",
            note: "",
        },
    ],
    notes: &[
        "the menu opens on the probe's `context_press`, which acts on the message under the pointer, so `move_pointer` onto one first",
    ],
};
const SLOW_DELAY: Duration = Duration::from_millis(1500);
pub const REFUSAL_DELAY: Duration = Duration::from_secs(1);

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Powers {
    #[default]
    Unrestricted,
    Member,
    Barred,
}

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub is_slow: bool,
    pub link_fails: bool,
    pub source_unavailable: bool,
    pub delete_refused: bool,
    pub delete_fails: bool,
    pub pin_fails: bool,
    pub powers: Powers,
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
        link_fails = scenario.link_fails,
        source_unavailable = scenario.source_unavailable,
        delete_refused = scenario.delete_refused,
        delete_fails = scenario.delete_fails,
        pin_fails = scenario.pin_fails,
        powers = ?scenario.powers,
        "demo mode: shaping the message menu"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" => scenario.is_slow = true,
        "link-fails" => scenario.link_fails = true,
        "source-unavailable" => scenario.source_unavailable = true,
        "delete-refused" => scenario.delete_refused = true,
        "delete-fails" => scenario.delete_fails = true,
        "pin-fails" => scenario.pin_fails = true,
        "member" => scenario.powers = Powers::Member,
        "barred" => scenario.powers = Powers::Barred,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub fn permissions() -> MessagePermissions {
    match scenario().powers {
        Powers::Unrestricted => MessagePermissions::UNRESTRICTED,
        Powers::Member => MessagePermissions {
            delete_own: true,
            delete_others: false,
            pin: false,
            notify_room: false,
        },
        Powers::Barred => MessagePermissions {
            delete_own: false,
            delete_others: false,
            pin: false,
            notify_room: false,
        },
    }
}

pub async fn pause() {
    if scenario().is_slow {
        sleep(SLOW_DELAY).await;
    }
}
