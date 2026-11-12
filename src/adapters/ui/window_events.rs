use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{EventResult, WinitWindowAccessor};

use super::activity::activity_watch;
use super::backend::UiBackend;
use super::call_window::take_token;
use super::context_press::context_press;
use super::focus::keeper::{FocusKeeper, step_focus};
use super::focus::{call_input_modality, focus_mirror, input_modality};
use super::swipe::reply_swipe;

type Activity = Rc<RefCell<dyn FnMut(&WindowEvent)>>;

thread_local! {
    static ACTIVITY: RefCell<Option<Activity>> = const { RefCell::new(None) };
}

pub fn install_window_events<B: UiBackend>(window: &B::Window) {
    let mut mirror = focus_mirror::<B>(window);
    let mut modality = input_modality::<B>(window);
    let activity: Activity = Rc::new(RefCell::new(activity_watch::<B>(window)));
    ACTIVITY.set(Some(Rc::clone(&activity)));
    let mut swipe = reply_swipe::<B>(window);
    let mut menu = context_press::<B>(window);
    let keeper = FocusKeeper::install(window, B::recover_focus);
    let stepped = window.as_weak();
    B::bind_focus_steps(window, move |forward| {
        if let Some(window) = stepped.upgrade() {
            step_focus(window.window(), forward);
        }
    });
    let main = window.as_weak();
    window
        .window()
        .on_winit_window_event(move |slint_window, event| {
            mirror(slint_window);
            modality(event);
            (activity.borrow_mut())(event);
            take_token::<B>(event, &main);
            menu(event);
            if keeper.take(slint_window, event) == EventResult::PreventDefault {
                return EventResult::PreventDefault;
            }
            swipe(slint_window, event)
        });
}

pub fn install_call_window_events<B: UiBackend>(call: &B::CallWindow, main: &B::Window) {
    let activity = ACTIVITY.with_borrow(Clone::clone);
    let main = main.as_weak();
    let mut modality = call_input_modality::<B>(call);
    let keeper = FocusKeeper::install(call, B::recover_call_focus);
    call.window()
        .on_winit_window_event(move |slint_window, event| {
            modality(event);
            if let Some(activity) = &activity {
                (activity.borrow_mut())(event);
            }
            take_token::<B>(event, &main);
            keeper.take(slint_window, event)
        });
}
