use std::collections::HashMap;
use std::env;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};
use super::data;
use crate::domain::member_name::NamedUser;
use crate::domain::room::RoomId;

const ENV_VAR: &str = "U2DM_DEMO_TYPING";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "makes members of the open room type, and shapes how your own typing notices answer",
    combinable: true,
    flags: &[
        Flag {
            value: "one",
            effect: "the newest other sender of the open room starts typing ~1s after it opens",
            note: "",
        },
        Flag {
            value: "two",
            effect: "the two newest other senders type",
            note: "",
        },
        Flag {
            value: "crowd",
            effect: "five members type: the newest senders, then the rest of the roster",
            note: "the line names the first and counts the others",
        },
        Flag {
            value: "bursts",
            effect: "the typists type for ~3s, stop for ~2s, and start again",
            note: "alone, one member types in bursts",
        },
        Flag {
            value: "notices-slow",
            effect: "each of your typing notices takes ~1.5s to answer",
            note: "",
        },
        Flag {
            value: "notices-fail",
            effect: "every typing notice of yours fails, which only a debug line shows",
            note: "",
        },
    ],
    notes: &[
        "a later flag of one, two and crowd adds no typists: the largest count wins",
        "your notices pass an imitation of matrix-sdk's throttle; each request it lets through logs `demo: a typing notice reaches the homeserver`",
        "with two and U2DM_DEMO_NAMES=clash, Vera and the impostor Kai type in !linux:demo.local, and the line names him by his user ID",
    ],
};
const START_DELAY: Duration = Duration::from_secs(1);
const BURST: Duration = Duration::from_secs(3);
const PAUSE: Duration = Duration::from_secs(2);
const SLOW_NOTICE: Duration = Duration::from_millis(1500);
const CROWD_SIZE: usize = 5;
const NOTICE_LIFETIME: Duration = Duration::from_secs(4);
const RESEND_AFTER: Duration = Duration::from_secs(3);

#[derive(Default, Clone, Copy)]
pub struct Scenario {
    pub typists: usize,
    pub in_bursts: bool,
    pub notices_slow: bool,
    pub notices_fail: bool,
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
    if scenario.in_bursts {
        scenario.typists = scenario.typists.max(1);
    }
    tracing::info!(
        typists = scenario.typists,
        in_bursts = scenario.in_bursts,
        notices_slow = scenario.notices_slow,
        notices_fail = scenario.notices_fail,
        "demo mode: shaping who types"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "one" => scenario.typists = scenario.typists.max(1),
        "two" => scenario.typists = scenario.typists.max(2),
        "crowd" => scenario.typists = scenario.typists.max(CROWD_SIZE),
        "bursts" => scenario.in_bursts = true,
        "notices-slow" => scenario.notices_slow = true,
        "notices-fail" => scenario.notices_fail = true,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub fn typists_in(room_id: &RoomId) -> Vec<NamedUser> {
    let wanted = scenario().typists;
    if wanted == 0 {
        return Vec::new();
    }
    let own = data::own_user();
    let mut users: Vec<String> = Vec::with_capacity(wanted);
    let senders = data::messages(room_id)
        .into_iter()
        .rev()
        .map(|message| message.sender);
    let roster = data::roster(room_id, str::to_owned)
        .into_iter()
        .map(|member| member.user_id);
    for user_id in senders.chain(roster) {
        if users.len() == wanted {
            break;
        }
        if user_id != own && !users.contains(&user_id) {
            users.push(user_id);
        }
    }
    let book = data::names_in(room_id, users.iter().cloned());
    users
        .into_iter()
        .map(|user_id| {
            let name = book.of(&user_id);
            NamedUser::new(user_id, name)
        })
        .collect()
}

pub async fn type_in_the_room(typists: Vec<NamedUser>, typists_tx: &mpsc::Sender<Vec<NamedUser>>) {
    sleep(START_DELAY).await;
    if !scenario().in_bursts {
        drop(typists_tx.send(typists).await);
        return;
    }
    loop {
        if typists_tx.send(typists.clone()).await.is_err() {
            return;
        }
        sleep(BURST).await;
        if typists_tx.send(Vec::new()).await.is_err() {
            return;
        }
        sleep(PAUSE).await;
    }
}

pub async fn pause_notice() {
    if scenario().notices_slow {
        sleep(SLOW_NOTICE).await;
    }
}

#[derive(Default)]
pub struct NoticeClock {
    sent: Mutex<HashMap<RoomId, Instant>>,
}

impl NoticeClock {
    pub fn notice(&self, room_id: &RoomId, typing: bool) {
        let Ok(mut sent) = self.sent.lock() else {
            return;
        };
        let reaches_homeserver = match sent.get(room_id).map(Instant::elapsed) {
            Some(elapsed) if elapsed > RESEND_AFTER => typing || elapsed <= NOTICE_LIFETIME,
            Some(_) => !typing,
            None => typing,
        };
        if !reaches_homeserver {
            return;
        }
        if typing {
            sent.insert(room_id.clone(), Instant::now());
        } else {
            sent.remove(room_id);
        }
        drop(sent);
        tracing::debug!(%room_id, typing, "demo: a typing notice reaches the homeserver");
    }
}
