use std::env;
use std::sync::{Arc, OnceLock};

use super::catalog::{Flag, Scenarios};
use crate::ports::appearance::{AppearancePort, MotionPreference, MotionSink};

const ENV_VAR: &str = "U2DM_DEMO_MOTION";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "stands in for the desktop's motion preference",
    combinable: false,
    flags: &[Flag {
        value: "reduced",
        effect: "the desktop asks for reduced motion: every movement (the drawer, the rail, the reaction picker, a settling swipe) jumps, fades still run",
        note: "a demo run never asks the real desktop",
    }],
    notes: &["unset, the desktop has no motion preference and every animation runs"],
};

#[derive(Clone, Copy)]
struct Scenario {
    motion: MotionPreference,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            motion: MotionPreference::Full,
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
        motion = ?scenario.motion,
        "demo mode: standing in for the desktop's motion preference"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "reduced" => scenario.motion = MotionPreference::Reduced,
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub fn port() -> Arc<dyn AppearancePort> {
    Arc::new(DemoAppearance)
}

struct DemoAppearance;

impl AppearancePort for DemoAppearance {
    fn follow_motion(&self, sink: MotionSink) {
        sink(scenario().motion);
    }
}
