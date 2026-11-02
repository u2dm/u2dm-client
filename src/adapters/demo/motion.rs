use std::sync::{Arc, OnceLock};

use super::catalog::{Flag, Scenarios, scenario_from_env};
use crate::ports::appearance::{AppearancePort, MotionPreference, MotionSink};

const ENV_VAR: &str = "U2DM_DEMO_MOTION";
const SHAPING: &str = "demo mode: standing in for the desktop's motion preference";

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
    *SCENARIO.get_or_init(|| scenario_from_env(ENV_VAR, apply, SHAPING))
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
