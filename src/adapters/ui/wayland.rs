use std::ffi::c_void;

use slint::Window;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};

pub fn wayland_display(window: &Window) -> Option<*mut c_void> {
    let handle = window.with_winit_window(|winit| {
        winit
            .display_handle()
            .ok()
            .map(|handle| handle.as_raw())
            .and_then(|handle| match handle {
                RawDisplayHandle::Wayland(wayland) => Some(wayland.display.as_ptr()),
                _ => None,
            })
    })??;
    Some(handle)
}
