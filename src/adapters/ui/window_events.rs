use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{EventResult, WinitWindowAccessor};

use super::activity::activity_watch;
use super::backend::UiBackend;
use super::call_window::take_token;
use super::context_press::context_press;
use super::focus::focus_mirror;
use super::swipe::reply_swipe;

type Activity = Rc<RefCell<dyn FnMut(&WindowEvent)>>;

thread_local! {
    static ACTIVITY: RefCell<Option<Activity>> = const { RefCell::new(None) };
}

pub fn install_window_events<B: UiBackend>(window: &B::Window) {
    let mut mirror = focus_mirror::<B>(window);
    let activity: Activity = Rc::new(RefCell::new(activity_watch::<B>(window)));
    ACTIVITY.set(Some(Rc::clone(&activity)));
    let mut swipe = reply_swipe::<B>(window);
    let mut menu = context_press::<B>(window);
    let main = window.as_weak();
    window
        .window()
        .on_winit_window_event(move |slint_window, event| {
            mirror(slint_window);
            (activity.borrow_mut())(event);
            take_token::<B>(event, &main);
            menu(event);
            swipe(slint_window, event)
        });
}

pub fn install_call_window_events<B: UiBackend>(call: &B::CallWindow, main: &B::Window) {
    let activity = ACTIVITY.with_borrow(Clone::clone);
    let main = main.as_weak();
    call.window().on_winit_window_event(move |_, event| {
        if let Some(activity) = &activity {
            (activity.borrow_mut())(event);
        }
        take_token::<B>(event, &main);
        EventResult::Propagate
    });
}
