pub mod keeper;
mod keys;

use std::cell::Cell;

use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::event::{ElementState, WindowEvent};
use slint::winit_030::winit::window::Window as WinitWindow;
use slint::{ComponentHandle, Window};

use self::keys::slint_text;
use super::backend::UiBackend;
use super::props::{BoolProp, UiProps};

pub fn input_modality<B: UiBackend>(window: &B::Window) -> impl FnMut(&WindowEvent) + use<B> {
    let weak = window.as_weak();
    follow_modality(move |keyboard| {
        if let Some(window) = weak.upgrade() {
            window.set_bool(BoolProp::KeyboardInUse, keyboard);
        }
    })
}

pub fn call_input_modality<B: UiBackend>(
    call: &B::CallWindow,
) -> impl FnMut(&WindowEvent) + use<B> {
    let weak = call.as_weak();
    follow_modality(move |keyboard| {
        if let Some(call) = weak.upgrade() {
            B::set_call_keyboard_in_use(&call, keyboard);
        }
    })
}

fn follow_modality(write: impl Fn(bool)) -> impl FnMut(&WindowEvent) {
    let mirrored = Cell::new(None);
    move |event: &WindowEvent| {
        let keyboard = match event {
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if event.state == ElementState::Pressed && slint_text(event).is_some() => true,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            }
            | WindowEvent::Touch(_) => false,
            _ => return,
        };
        if mirrored.get() != Some(keyboard) {
            mirrored.set(Some(keyboard));
            write(keyboard);
        }
    }
}

pub fn focus_mirror<B: UiBackend>(window: &B::Window) -> impl FnMut(&Window) + use<B> {
    let weak = window.as_weak();
    let mirrored = Cell::new(None);
    move |slint_window: &Window| {
        let focused = slint_window.with_winit_window(WinitWindow::has_focus);
        if focused != mirrored.get()
            && let (Some(focused), Some(window)) = (focused, weak.upgrade())
        {
            mirrored.set(Some(focused));
            tracing::debug!(focused, "the window's focus changed");
            window.set_bool(BoolProp::WindowFocused, focused);
            if focused {
                slint_window.with_winit_window(|winit| winit.request_user_attention(None));
            }
        }
    }
}
