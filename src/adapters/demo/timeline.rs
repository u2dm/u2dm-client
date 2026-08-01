use std::env;
use std::sync::OnceLock;
use std::time::Duration;

const ENV_VAR: &str = "U2DM_DEMO_TIMELINE";

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
            };
        }
        other => tracing::warn!(flag = other, "demo mode: unknown timeline scenario flag"),
    }
}
