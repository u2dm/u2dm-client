use std::env;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::time::sleep;

use super::catalog::{Flag, Scenarios};
use super::data;
use crate::domain::presence::{
    Availability, BeaconFailure, BeaconResult, LookupFailure, LookupResult, PresenceReport,
};
use crate::domain::user_info::UserId;

const ENV_VAR: &str = "U2DM_DEMO_PRESENCE";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "shapes what the homeserver answers about presence, yours and the people you look at",
    combinable: true,
    flags: &[
        Flag {
            value: "partners",
            effect: "presence is on: Sarah is online, Priya away, Nina was last seen 3 hours ago, anyone else 2 days ago",
            note: "your own lookups answer what you last announced",
        },
        Flag {
            value: "status",
            effect: "Sarah and Priya carry status messages, Priya's right to left, and so do you",
            note: "your beats carry your status back unchanged",
        },
        Flag {
            value: "changing",
            effect: "each lookup moves a partner one step: online, away, last seen, online",
            note: "",
        },
        Flag {
            value: "slow",
            effect: "every presence answer takes ~1.5s",
            note: "",
        },
        Flag {
            value: "refused",
            effect: "every presence call answers 403, as tuwunel does with presence off",
            note: "",
        },
        Flag {
            value: "throttled",
            effect: "your announcements are limited to one per 10s, as Synapse's rc_presence does",
            note: "a sooner one answers 429 with the time left",
        },
        Flag {
            value: "flaky",
            effect: "every second presence call gets no answer",
            note: "",
        },
    ],
    notes: &[
        "unset, the homeserver behaves like matrix.org: announcements are accepted and every lookup answers offline with nothing else, so the beacon stops after its second beat and no line shows",
        "each announcement the homeserver takes logs `demo: presence reaches the homeserver`",
    ],
};
const SLOW_ANSWER: Duration = Duration::from_millis(1500);
const THROTTLE_GAP: Duration = Duration::from_secs(10);
const SARAH: &str = "@sarah:matrix.org";
const PRIYA: &str = "@priya:matrix.org";
const NINA: &str = "@nina:matrix.org";
const PRIYA_AWAY_FOR: Duration = Duration::from_mins(12);
const NINA_SEEN_AGO: Duration = Duration::from_hours(3);
const OTHERS_SEEN_AGO: Duration = Duration::from_hours(48);
const SARAH_STATUS: &str = "In a meeting until 3";
const PRIYA_STATUS: &str = "בחופשה עד יום שני";
const OWN_STATUS: &str = "Writing the demo";

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    partners: bool,
    status: bool,
    changing: bool,
    slow: bool,
    refused: bool,
    throttled: bool,
    flaky: bool,
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
        partners = scenario.partners,
        status = scenario.status,
        changing = scenario.changing,
        slow = scenario.slow,
        refused = scenario.refused,
        throttled = scenario.throttled,
        flaky = scenario.flaky,
        "demo mode: shaping presence"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "partners" => scenario.partners = true,
        "status" => scenario.status = true,
        "changing" => scenario.changing = true,
        "slow" => scenario.slow = true,
        "refused" => scenario.refused = true,
        "throttled" => scenario.throttled = true,
        "flaky" => scenario.flaky = true,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

#[derive(Default)]
pub struct PresenceBook {
    book: Mutex<Book>,
}

#[derive(Default)]
struct Book {
    announced: Option<Announced>,
    accepted_at: Option<Instant>,
    calls: u64,
    steps: u64,
}

struct Announced {
    availability: Availability,
    status: Option<String>,
    at: Instant,
}

enum Gate {
    Answers,
    Refused,
    NoAnswer,
}

impl PresenceBook {
    pub async fn announce(&self, availability: Availability, status: Option<&str>) -> BeaconResult {
        match self.gate().await {
            Gate::Refused => return Err(BeaconFailure::Refused),
            Gate::NoAnswer => return Err(BeaconFailure::Failed),
            Gate::Answers => {}
        }
        let Ok(mut book) = self.book.lock() else {
            return Err(BeaconFailure::Failed);
        };
        let now = Instant::now();
        if scenario().throttled
            && let Some(wait) = book
                .accepted_at
                .map(|at| THROTTLE_GAP.saturating_sub(now.duration_since(at)))
                .filter(|wait| !wait.is_zero())
        {
            tracing::debug!(?availability, ?wait, "demo: presence refused for its rate");
            return Err(BeaconFailure::Throttled(Some(wait)));
        }
        book.accepted_at = Some(now);
        book.announced = Some(Announced {
            availability,
            status: status.map(str::to_owned),
            at: now,
        });
        drop(book);
        tracing::debug!(
            ?availability,
            status,
            "demo: presence reaches the homeserver"
        );
        Ok(())
    }

    pub async fn presence_of(&self, user_id: &UserId) -> LookupResult {
        match self.gate().await {
            Gate::Refused => return Err(LookupFailure::Refused),
            Gate::NoAnswer => return Err(LookupFailure::Failed),
            Gate::Answers => {}
        }
        let shaped = scenario();
        if !shaped.partners {
            return Ok(silent());
        }
        let Ok(mut book) = self.book.lock() else {
            return Err(LookupFailure::Failed);
        };
        if user_id.as_ref() == data::own_user() {
            return Ok(own_report(book.announced.as_ref(), shaped.status));
        }
        let step = if shaped.changing && is_partner(user_id) {
            book.steps = book.steps.wrapping_add(1);
            book.steps
        } else {
            0
        };
        drop(book);
        Ok(partner_report(user_id, step, shaped.status))
    }

    async fn gate(&self) -> Gate {
        let shaped = scenario();
        if shaped.slow {
            sleep(SLOW_ANSWER).await;
        }
        if shaped.refused {
            return Gate::Refused;
        }
        let call = self.book.lock().map_or(0, |mut book| {
            book.calls = book.calls.wrapping_add(1);
            book.calls
        });
        if shaped.flaky && call.is_multiple_of(2) {
            return Gate::NoAnswer;
        }
        Gate::Answers
    }
}

fn silent() -> PresenceReport {
    PresenceReport {
        availability: Some(Availability::Offline),
        ..PresenceReport::default()
    }
}

fn own_report(announced: Option<&Announced>, with_status: bool) -> PresenceReport {
    let fallback_status = with_status.then(|| OWN_STATUS.to_owned());
    let Some(announced) = announced else {
        return PresenceReport {
            status: fallback_status,
            ..silent()
        };
    };
    PresenceReport {
        availability: Some(announced.availability),
        currently_active: announced.availability == Availability::Online,
        last_active_ago: Some(announced.at.elapsed()),
        status: announced.status.clone().or(fallback_status),
    }
}

fn is_partner(user_id: &UserId) -> bool {
    [SARAH, PRIYA, NINA].contains(&user_id.as_ref())
}

fn partner_report(user_id: &UserId, step: u64, with_status: bool) -> PresenceReport {
    let (start, status): (u64, Option<&str>) = match user_id.as_ref() {
        SARAH => (0, Some(SARAH_STATUS)),
        PRIYA => (1, Some(PRIYA_STATUS)),
        NINA => (2, None),
        _ => {
            return PresenceReport {
                availability: Some(Availability::Offline),
                last_active_ago: Some(OTHERS_SEEN_AGO),
                ..PresenceReport::default()
            };
        }
    };
    let status = status.filter(|_| with_status).map(str::to_owned);
    match start.wrapping_add(step) % 3 {
        0 => PresenceReport {
            availability: Some(Availability::Online),
            currently_active: true,
            last_active_ago: Some(Duration::ZERO),
            status,
        },
        1 => PresenceReport {
            availability: Some(Availability::Unavailable),
            currently_active: false,
            last_active_ago: Some(PRIYA_AWAY_FOR),
            status,
        },
        _ => PresenceReport {
            availability: Some(Availability::Offline),
            currently_active: false,
            last_active_ago: Some(NINA_SEEN_AGO),
            status,
        },
    }
}
