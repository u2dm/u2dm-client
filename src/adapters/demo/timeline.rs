use super::catalog::{Flag, Scenarios};
use std::env;
use std::sync::OnceLock;
use std::time::Duration;

const ENV_VAR: &str = "U2DM_DEMO_TIMELINE";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "reproduces real-homeserver timeline delivery timing",
    combinable: true,
    flags: &[
        Flag {
            value: "deep",
            effect: "six copies of history with 60% unread",
            note: "include in ANY anchor or unread test; without it the unread run fits on screen and every other flag looks like it passes",
        },
        Flag {
            value: "slow",
            effect: "the Reset lands 1.5s after SelectedRoom",
            note: "",
        },
        Flag {
            value: "batch",
            effect: "the Reset arrives nested in a Batch",
            note: "",
        },
        Flag {
            value: "late-reset",
            effect: "a second Reset follows the first, as the event cache replaces its events",
            note: "",
        },
        Flag {
            value: "drop-anchor",
            effect: "that second Reset arrives too short to hold the anchor",
            note: "deliberately excluded from `all`; asserts the timeline stays put rather than jumping to the end",
        },
        Flag {
            value: "churn",
            effect: "rows keep resizing for another second",
            note: "",
        },
        Flag {
            value: "append",
            effect: "somebody posts while the timeline is still settling",
            note: "",
        },
        Flag {
            value: "prepend",
            effect: "back-pagination returns history, so every row index shifts",
            note: "",
        },
        Flag {
            value: "sync",
            effect: "the room list updates every 400ms, re-emitting SelectedRoom mid-flight",
            note: "",
        },
        Flag {
            value: "all-unread",
            effect: "the read position precedes everything loaded, so the anchor is row 0",
            note: "excluded from `all`",
        },
        Flag {
            value: "unread-resolving",
            effect: "holds the \"Loading unread messages...\" pill while the read position is placed",
            note: "excluded from `all`",
        },
        Flag {
            value: "jump-far",
            effect: "the live window holds only the newest few messages, forcing a focused /context jump",
            note: "excluded from `all`",
        },
        Flag {
            value: "send-fails",
            effect: "every message sent is wedged in the queue, so the retry and discard row renders",
            note: "excluded from `all`",
        },
        Flag {
            value: "all",
            effect: "slow, batch, late-reset, churn, sync, append, prepend and deep",
            note: "excludes drop-anchor, all-unread, unread-resolving and jump-far",
        },
    ],
    notes: &["unknown flags warn and are ignored, never rejected"],
};

pub const SLOW_RESET_DELAY: Duration = Duration::from_millis(1500);
pub const REPEATED_RESET_DELAY: Duration = Duration::from_millis(700);
pub const RESIZE_INTERVAL: Duration = Duration::from_millis(150);
pub const RESIZE_ROUNDS: usize = 8;
pub const ROOM_LIST_INTERVAL: Duration = Duration::from_millis(400);
pub const LATE_MESSAGE_DELAY: Duration = Duration::from_millis(300);
pub const HISTORY_PAGE: usize = 12;
pub const FOCUS_CONTEXT: usize = 15;
pub const SHORT_WINDOW: usize = 8;
pub const HISTORY_COPIES: usize = 6;
pub const UNREAD_PORTION_NUMERATOR: usize = 3;
pub const UNREAD_PORTION_DENOMINATOR: usize = 5;

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub reset_is_slow: bool,
    pub reset_is_batched: bool,
    pub reset_repeats: bool,
    pub repeat_drops_anchor: bool,
    pub rows_keep_resizing: bool,
    pub room_list_keeps_updating: bool,
    pub message_arrives_late: bool,
    pub pagination_returns_history: bool,
    pub history_is_long: bool,
    pub read_position_precedes_history: bool,
    pub resolving_unread: bool,
    pub window_is_short: bool,
    pub sends_fail: bool,
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
        reset_is_slow = scenario.reset_is_slow,
        reset_is_batched = scenario.reset_is_batched,
        reset_repeats = scenario.reset_repeats,
        repeat_drops_anchor = scenario.repeat_drops_anchor,
        rows_keep_resizing = scenario.rows_keep_resizing,
        room_list_keeps_updating = scenario.room_list_keeps_updating,
        message_arrives_late = scenario.message_arrives_late,
        pagination_returns_history = scenario.pagination_returns_history,
        history_is_long = scenario.history_is_long,
        read_position_precedes_history = scenario.read_position_precedes_history,
        resolving_unread = scenario.resolving_unread,
        window_is_short = scenario.window_is_short,
        sends_fail = scenario.sends_fail,
        "demo mode: reproducing real-account timeline timing"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" => scenario.reset_is_slow = true,
        "batch" => scenario.reset_is_batched = true,
        "late-reset" => scenario.reset_repeats = true,
        "drop-anchor" => {
            scenario.reset_repeats = true;
            scenario.repeat_drops_anchor = true;
        }
        "churn" => scenario.rows_keep_resizing = true,
        "append" => scenario.message_arrives_late = true,
        "prepend" => scenario.pagination_returns_history = true,
        "sync" => scenario.room_list_keeps_updating = true,
        "deep" => scenario.history_is_long = true,
        "all-unread" => scenario.read_position_precedes_history = true,
        "unread-resolving" => scenario.resolving_unread = true,
        "jump-far" => scenario.window_is_short = true,
        "send-fails" => scenario.sends_fail = true,
        "all" => {
            *scenario = Scenario {
                reset_is_slow: true,
                reset_is_batched: true,
                reset_repeats: true,
                repeat_drops_anchor: false,
                rows_keep_resizing: true,
                room_list_keeps_updating: true,
                message_arrives_late: true,
                pagination_returns_history: true,
                history_is_long: true,
                read_position_precedes_history: false,
                resolving_unread: false,
                window_is_short: false,
                sends_fail: false,
            };
        }
        other => tracing::warn!(flag = other, "demo mode: unknown timeline scenario flag"),
    }
}
