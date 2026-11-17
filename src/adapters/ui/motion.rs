use std::cell::Cell;
use std::sync::Arc;

use i_slint_core::MotionPreference as SlintMotion;
use slint::private_unstable_api::ensure_context;

use super::backend::{UiBackend, in_event_loop};
use super::props::{BoolProp, UiProps};
use crate::domain::settings::MotionChoice;
use crate::ports::appearance::{AppearancePort, MotionPreference};

thread_local! {
    static MOTION_INPUTS: Cell<(MotionChoice, MotionPreference)> =
        const { Cell::new((MotionChoice::System, MotionPreference::Full)) };
}

pub fn follow_motion<B: UiBackend>(window: &B::Window, appearance: &dyn AppearancePort) {
    appearance.follow_motion(Arc::new(in_event_loop::<B, MotionPreference>(
        window,
        "a motion preference",
        |window, preference| {
            let reduced = preference == MotionPreference::Reduced;
            tracing::debug!(reduced, "the UI follows the desktop's motion preference");
            window.set_bool(BoolProp::ReducedMotion, reduced);
            MOTION_INPUTS.with(|inputs| inputs.set((inputs.get().0, preference)));
            tell_slint();
        },
    )));
}

pub fn follow_choice(choice: MotionChoice) {
    MOTION_INPUTS.with(|inputs| inputs.set((choice, inputs.get().1)));
    tell_slint();
}

fn reduces_motion(choice: MotionChoice, desktop: MotionPreference) -> bool {
    match choice {
        MotionChoice::Reduced => true,
        MotionChoice::Full => false,
        MotionChoice::System => desktop == MotionPreference::Reduced,
    }
}

fn tell_slint() {
    let (choice, desktop) = MOTION_INPUTS.with(Cell::get);
    let preference = if reduces_motion(choice, desktop) {
        SlintMotion::Reduced
    } else {
        SlintMotion::NoPreference
    };
    match ensure_context() {
        Ok(context) => context.set_motion_preference(preference),
        Err(e) => tracing::warn!("Slint's own scrolling cannot follow the motion preference: {e}"),
    }
}
