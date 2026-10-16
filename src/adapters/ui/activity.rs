use std::time::Duration;

use slint::winit_030::winit::event::{Ime, WindowEvent};
use slint::{ComponentHandle, Timer, TimerMode};

use super::backend::UiBackend;
use super::props::{BoolProp, UiProps};

const IDLE_AFTER: Duration = Duration::from_mins(5);

pub fn activity_watch<B: UiBackend>(window: &B::Window) -> impl FnMut(&WindowEvent) + use<B> {
    let weak = window.as_weak();
    let idle = Timer::default();
    let mut armed = false;
    move |event| {
        let input = is_user_input(event);
        if input && let Some(window) = weak.upgrade() {
            report(&window, true);
        }
        if input && idle.running() {
            idle.restart();
        } else if input || !armed {
            armed = true;
            let weak = weak.clone();
            idle.start(TimerMode::SingleShot, IDLE_AFTER, move || {
                if let Some(window) = weak.upgrade() {
                    report(&window, false);
                }
            });
        }
    }
}

pub fn report(window: &(impl ComponentHandle + UiProps), active: bool) {
    if window.get_bool(BoolProp::WindowActive) == active {
        return;
    }
    tracing::debug!(active, "the window's activity changed");
    window.set_bool(BoolProp::WindowActive, active);
    window.window().request_redraw();
}

const fn is_user_input(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::KeyboardInput { .. }
            | WindowEvent::Ime(Ime::Preedit(..) | Ime::Commit(_))
            | WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::CursorMoved { .. }
            | WindowEvent::Touch(_)
            | WindowEvent::PinchGesture { .. }
            | WindowEvent::PanGesture { .. }
            | WindowEvent::DoubleTapGesture { .. }
            | WindowEvent::RotationGesture { .. }
            | WindowEvent::Focused(true)
    )
}
