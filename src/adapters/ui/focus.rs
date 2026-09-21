use std::cell::Cell;

use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::Window as WinitWindow;
use slint::{ComponentHandle, Window};

use super::backend::UiBackend;
use super::props::{BoolProp, UiProps};

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
        }
    }
}
