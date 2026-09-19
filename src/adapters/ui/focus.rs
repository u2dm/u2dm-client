use std::cell::Cell;

use slint::ComponentHandle;
use slint::winit_030::winit::window::Window as WinitWindow;
use slint::winit_030::{EventResult, WinitWindowAccessor};

use super::backend::UiBackend;
use super::props::{BoolProp, UiProps};

pub fn install_focus_mirror<B: UiBackend>(window: &B::Window) {
    let weak = window.as_weak();
    let mirrored = Cell::new(None);
    window
        .window()
        .on_winit_window_event(move |slint_window, _| {
            let focused = slint_window.with_winit_window(WinitWindow::has_focus);
            if focused != mirrored.get()
                && let (Some(focused), Some(window)) = (focused, weak.upgrade())
            {
                mirrored.set(Some(focused));
                tracing::debug!(focused, "the window's focus changed");
                window.set_bool(BoolProp::WindowFocused, focused);
            }
            EventResult::Propagate
        });
}
