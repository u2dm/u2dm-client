use std::collections::HashSet;
use std::env;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};

const ENV_VAR: &str = "U2DM_DEMO_SPACE_INDEX";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "shapes the space index: its /hierarchy pages, joins and the avatars of unjoined rooms",
    combinable: true,
    flags: &[
        Flag {
            value: "slow",
            effect: "each page and each join waits ~1.5s, and an unjoined room's avatar exists only once its fetch batch lands",
            note: "the loading, loading-more and joining states are only reachable with `slow`",
        },
        Flag {
            value: "fail",
            effect: "the first page of each space fails once, so the retry state is reachable",
            note: "",
        },
        Flag {
            value: "more-fails",
            effect: "the second page of each space fails once, so the footer retry is reachable",
            note: "",
        },
        Flag {
            value: "join-fails",
            effect: "every join fails, so the failure toast is reachable",
            note: "",
        },
    ],
    notes: &[
        "pages hold 6 rows; the Matrix space spans three of them",
        "a join returns first and reaches the room list ~0.8s later, as a real sync echo does",
    ],
};
pub const PAGE_SIZE: usize = 6;
pub const JOIN_ECHO_LAG: Duration = Duration::from_millis(800);
const SLOW_DELAY: Duration = Duration::from_millis(1500);
const AVATAR_TRICKLE: Duration = Duration::from_millis(400);

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub is_slow: bool,
    pub first_page_fails: bool,
    pub next_page_fails: bool,
    pub join_fails: bool,
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
        first_page_fails = scenario.first_page_fails,
        next_page_fails = scenario.next_page_fails,
        join_fails = scenario.join_fails,
        "demo mode: shaping the space index"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "slow" => scenario.is_slow = true,
        "fail" => scenario.first_page_fails = true,
        "more-fails" => scenario.next_page_fails = true,
        "join-fails" => scenario.join_fails = true,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub async fn pause() {
    if scenario().is_slow {
        sleep(SLOW_DELAY).await;
    }
}

pub async fn pause_avatars() {
    if scenario().is_slow {
        sleep(AVATAR_TRICKLE).await;
    }
}

fn failed_pages() -> &'static Mutex<HashSet<(String, usize)>> {
    static FAILED: OnceLock<Mutex<HashSet<(String, usize)>>> = OnceLock::new();
    FAILED.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn page_fails_now(space_id: &str, offset: usize) -> bool {
    let demo = scenario();
    let scripted = if offset == 0 {
        demo.first_page_fails
    } else {
        demo.next_page_fails && offset == PAGE_SIZE
    };
    if !scripted {
        return false;
    }
    failed_pages()
        .lock()
        .is_ok_and(|mut failed| failed.insert((space_id.to_owned(), offset)))
}
