use std::sync::Arc;

use slint::ComponentHandle;

use super::backend::UiBackend;
use super::props::{BoolProp, UiProps};
use crate::ports::appearance::{AppearancePort, MotionPreference};

pub fn follow_motion<B: UiBackend>(window: &B::Window, appearance: &dyn AppearancePort) {
    let weak = window.as_weak();
    appearance.follow_motion(Arc::new(move |preference: MotionPreference| {
        let queued = weak.upgrade_in_event_loop(move |window| {
            let reduced = preference == MotionPreference::Reduced;
            tracing::debug!(reduced, "the UI follows the desktop's motion preference");
            window.set_bool(BoolProp::ReducedMotion, reduced);
        });
        if let Err(e) = queued {
            tracing::debug!("a motion preference could not reach the event loop: {e}");
        }
    }));
}
