use std::time::Duration;

use slint::winit_030::winit::event::{ElementState, MouseButton, WindowEvent};
use slint::{ComponentHandle, Timer};

use super::backend::UiBackend;
use super::props::{IntProp, UiProps};

pub fn context_press<B: UiBackend>(window: &B::Window) -> impl FnMut(&WindowEvent) + use<B> {
    let weak = window.as_weak();
    move |event| {
        if is_secondary_press(event) {
            let weak = weak.clone();
            Timer::single_shot(Duration::ZERO, move || {
                if let Some(window) = weak.upgrade() {
                    announce(&window);
                }
            });
        }
    }
}

pub fn announce(window: &(impl ComponentHandle + UiProps)) {
    window.bump(IntProp::ContextPresses);
    window.window().request_redraw();
}

const fn is_secondary_press(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::MouseInput {
            state: ElementState::Pressed,
            button: MouseButton::Right,
            ..
        }
    )
}
