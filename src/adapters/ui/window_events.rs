use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;

use super::backend::UiBackend;
use super::focus::focus_mirror;
use super::swipe::reply_swipe;

pub fn install_window_events<B: UiBackend>(window: &B::Window) {
    let mut mirror = focus_mirror::<B>(window);
    let mut swipe = reply_swipe::<B>(window);
    window
        .window()
        .on_winit_window_event(move |slint_window, event| {
            mirror(slint_window);
            swipe(slint_window, event)
        });
}
