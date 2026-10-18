use std::env;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::catalog::{Flag, Scenarios};
use super::{message_menu, polls};
use crate::domain::message::MessagePermissions;
use crate::domain::poll::PollPermissions;
use crate::error::{AppError, Result};

const ENV_VAR: &str = "U2DM_DEMO_POWERS";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "sets what every room's power levels let you send",
    combinable: true,
    flags: &[
        Flag {
            value: "read-only",
            effect: "events_default is above your level: no messages, stickers, reactions, polls or deletions",
            note: "the composer turns into its notice",
        },
        Flag {
            value: "reactions-only",
            effect: "an announcement channel that lets everyone react: reactions and taking them back, nothing else",
            note: "",
        },
        Flag {
            value: "no-messages",
            effect: "m.room.message alone is above your level, so stickers, polls and reactions stay",
            note: "the composer keeps the sticker and poll buttons beside its notice",
        },
        Flag {
            value: "no-reactions",
            effect: "m.reaction alone is above your level",
            note: "",
        },
        Flag {
            value: "no-stickers",
            effect: "m.sticker alone is above your level",
            note: "",
        },
        Flag {
            value: "revoked",
            effect: "you may send everything until 5s after a room opens, then a room list update makes every room read-only",
            note: "the change arrives through the room list, as a power-level change does",
        },
        Flag {
            value: "granted",
            effect: "every room is read-only until 5s after a room opens, then a room list update lets you send everything",
            note: "the composer comes back with the text and reply it held",
        },
    ],
    notes: &[
        "the later flag wins",
        "U2DM_DEMO_MESSAGE_MENU and U2DM_DEMO_POLLS still bar their own actions on top",
    ],
};

pub const SWITCH_AFTER: Duration = Duration::from_secs(5);

static SWITCHED: AtomicBool = AtomicBool::new(false);

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
enum Levels {
    #[default]
    Unrestricted,
    ReadOnly,
    ReactionsOnly,
    NoMessages,
    NoReactions,
    NoStickers,
}

#[derive(Default, Clone, Copy)]
struct Scenario {
    opening: Levels,
    switched_to: Option<Levels>,
}

fn scenario() -> Scenario {
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
        opening = ?scenario.opening,
        switched_to = ?scenario.switched_to,
        "demo mode: shaping what the power levels let you send"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    let (opening, switched_to) = match flag {
        "read-only" => (Levels::ReadOnly, None),
        "reactions-only" => (Levels::ReactionsOnly, None),
        "no-messages" => (Levels::NoMessages, None),
        "no-reactions" => (Levels::NoReactions, None),
        "no-stickers" => (Levels::NoStickers, None),
        "revoked" => (Levels::Unrestricted, Some(Levels::ReadOnly)),
        "granted" => (Levels::ReadOnly, Some(Levels::Unrestricted)),
        other => {
            tracing::warn!("unknown {ENV_VAR} flag: {other}");
            return;
        }
    };
    *scenario = Scenario {
        opening,
        switched_to,
    };
}

fn current() -> Levels {
    let scenario = scenario();
    match scenario.switched_to {
        Some(levels) if SWITCHED.load(Ordering::Relaxed) => levels,
        _ => scenario.opening,
    }
}

pub fn messages() -> MessagePermissions {
    let barred = message_menu::permissions();
    match current() {
        Levels::Unrestricted => barred,
        Levels::ReadOnly => MessagePermissions {
            send_messages: false,
            react: false,
            send_stickers: false,
            delete_own: false,
            delete_others: false,
            ..barred
        },
        Levels::ReactionsOnly => MessagePermissions {
            send_messages: false,
            send_stickers: false,
            delete_others: false,
            ..barred
        },
        Levels::NoMessages => MessagePermissions {
            send_messages: false,
            ..barred
        },
        Levels::NoReactions => MessagePermissions {
            react: false,
            ..barred
        },
        Levels::NoStickers => MessagePermissions {
            send_stickers: false,
            ..barred
        },
    }
}

pub fn polls() -> PollPermissions {
    match current() {
        Levels::ReadOnly | Levels::ReactionsOnly => PollPermissions {
            vote: false,
            end: false,
            start: false,
        },
        Levels::Unrestricted | Levels::NoMessages | Levels::NoReactions | Levels::NoStickers => {
            polls::permissions()
        }
    }
}

pub fn refuse_unless(allowed: bool, what: &str) -> Result<()> {
    if allowed {
        return Ok(());
    }
    tracing::info!(what, "demo: the power levels refuse it");
    Err(AppError::SendNotAllowed)
}

pub fn switch() -> bool {
    scenario().switched_to.is_some() && !SWITCHED.swap(true, Ordering::Relaxed)
}
