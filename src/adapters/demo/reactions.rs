use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use crate::domain::message::{Reaction, TimelineMessage};

const ENV_VAR: &str = "U2DM_DEMO_REACTIONS";

pub const LATE_INTERVAL: Duration = Duration::from_millis(350);

const LONG_KEY: &str = "this reaction key is a whole sentence and has to be elided";
const CROWD_SIZE: usize = 40;
const OVERFLOW_KEYS: &[&str] = &["🎉", "👀", "🚀", "😕", "🔥", "🐢", "🥲", "🫡", "🧊"];

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub reactions_arrive_late: bool,
    pub keys_overflow: bool,
    pub key_is_long: bool,
    pub mine_stays_pending: bool,
    pub crowd_reacts: bool,
    pub toggle_has_no_echo: bool,
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
        reactions_arrive_late = scenario.reactions_arrive_late,
        keys_overflow = scenario.keys_overflow,
        key_is_long = scenario.key_is_long,
        mine_stays_pending = scenario.mine_stays_pending,
        crowd_reacts = scenario.crowd_reacts,
        toggle_has_no_echo = scenario.toggle_has_no_echo,
        "demo mode: reproducing real-account reaction timing"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "late" => scenario.reactions_arrive_late = true,
        "overflow" => scenario.keys_overflow = true,
        "long-key" => scenario.key_is_long = true,
        "pending" => scenario.mine_stays_pending = true,
        "crowd" => scenario.crowd_reacts = true,
        "no-echo" => scenario.toggle_has_no_echo = true,
        "all" => {
            scenario.reactions_arrive_late = true;
            scenario.keys_overflow = true;
            scenario.key_is_long = true;
            scenario.crowd_reacts = true;
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

fn crowd_senders() -> Vec<String> {
    (0..CROWD_SIZE)
        .map(|n| format!("@demo{n}:matrix.org"))
        .collect()
}

fn newest_reacted_message(messages: &mut [TimelineMessage]) -> Option<&mut TimelineMessage> {
    messages
        .iter_mut()
        .rev()
        .find(|message| !message.reactions.is_empty())
}

fn carries_key(message: &TimelineMessage, key: &str) -> bool {
    message.reactions.iter().any(|reaction| reaction.key == key)
}

pub fn apply_scenario(messages: &mut [TimelineMessage]) {
    let scenario = scenario();
    if !scenario.keys_overflow && !scenario.key_is_long && !scenario.crowd_reacts {
        return;
    }
    let Some(message) = newest_reacted_message(messages) else {
        return;
    };
    if scenario.crowd_reacts
        && let Some(first) = message.reactions.first_mut()
    {
        first.senders = crowd_senders();
    }
    if scenario.key_is_long {
        message.reactions.push(Reaction {
            key: LONG_KEY.to_owned(),
            senders: vec!["@kai:matrix.org".to_owned()],
            mine: false,
            pending: false,
        });
    }
    if scenario.keys_overflow {
        for key in OVERFLOW_KEYS {
            if carries_key(message, key) {
                continue;
            }
            message.reactions.push(Reaction {
                key: (*key).to_owned(),
                senders: vec!["@priya:matrix.org".to_owned()],
                mine: false,
                pending: false,
            });
        }
    }
}

pub fn strip_reactions(messages: &[TimelineMessage]) -> Vec<TimelineMessage> {
    messages
        .iter()
        .map(|message| TimelineMessage {
            reactions: Vec::new(),
            ..message.clone()
        })
        .collect()
}

pub fn reacted_indices(messages: &[TimelineMessage]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, message)| !message.reactions.is_empty())
        .map(|(index, _)| index)
        .collect()
}

pub fn toggle(message: &mut TimelineMessage, key: &str, own_user: &str) {
    let pending = scenario().mine_stays_pending;
    let Some(position) = message.reactions.iter().position(|r| r.key == key) else {
        message.reactions.push(Reaction {
            key: key.to_owned(),
            senders: vec![own_user.to_owned()],
            mine: true,
            pending,
        });
        return;
    };
    let Some(reaction) = message.reactions.get_mut(position) else {
        return;
    };
    if let Some(mine) = reaction.senders.iter().position(|s| s == own_user) {
        reaction.senders.remove(mine);
        reaction.mine = false;
        reaction.pending = false;
        if reaction.senders.is_empty() {
            message.reactions.remove(position);
        }
        return;
    }
    reaction.senders.push(own_user.to_owned());
    reaction.mine = true;
    reaction.pending = pending;
}
