use std::sync::Arc;

use super::backend::{UiBackend, in_event_loop};
use super::props::{BoolProp, UiProps};
use crate::ports::appearance::{AppearancePort, MotionPreference};

pub fn follow_motion<B: UiBackend>(window: &B::Window, appearance: &dyn AppearancePort) {
    appearance.follow_motion(Arc::new(in_event_loop::<B, MotionPreference>(
        window,
        "a motion preference",
        |window, preference| {
            let reduced = preference == MotionPreference::Reduced;
            tracing::debug!(reduced, "the UI follows the desktop's motion preference");
            window.set_bool(BoolProp::ReducedMotion, reduced);
        },
    )));
}
