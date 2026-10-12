use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::catalog::{Flag, Scenarios};
use crate::domain::sync::{SyncEvent, SyncOutcome, SyncSupport};
use crate::ports::matrix::SyncSink;

const ENV_VAR: &str = "U2DM_DEMO_SYNC";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "shapes what the homeserver offers for syncing",
    combinable: true,
    flags: &[Flag {
        value: "no-sliding-sync",
        effect: "the homeserver offers only /v3/sync: discovery reports it, and a restored session's sync is refused every 4s",
        note: "with U2DM_DEMO_LOGIN, the homeserver step refuses to sign in; without, the chat opens on the drawer status and the toast",
    }],
    notes: &["unset, the homeserver offers simplified sliding sync (MSC4186)"],
};
const REFUSAL_INTERVAL: Duration = Duration::from_secs(4);

#[derive(Clone, Copy)]
struct Scenario {
    support: SyncSupport,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            support: SyncSupport::SlidingSync,
        }
    }
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
        support = ?scenario.support,
        "demo mode: shaping what the homeserver offers for syncing"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "no-sliding-sync" => scenario.support = SyncSupport::ClassicOnly,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub fn support() -> SyncSupport {
    scenario().support
}

pub async fn refuse_forever(on_sync: &SyncSink, cancel: &CancellationToken) -> SyncOutcome {
    tracing::info!("demo mode: the homeserver offers no sliding sync, so every sync is refused");
    loop {
        on_sync(SyncEvent::SlidingSyncUnsupported);
        tokio::select! {
            () = cancel.cancelled() => return SyncOutcome::Cancelled,
            () = sleep(REFUSAL_INTERVAL) => {}
        }
    }
}
