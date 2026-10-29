use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::time::Duration;

#[cfg(feature = "demo")]
use slint::platform::WindowEvent as PlatformEvent;
#[cfg(target_os = "linux")]
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::winit::event_loop::AsyncRequestSerial;
#[cfg(target_os = "linux")]
use slint::winit_030::winit::platform::startup_notify::WindowExtStartupNotify;
use slint::{CloseRequestResponse, ComponentHandle, Timer, Window};

use super::activation::raise;
use super::backend::UiBackend;
use super::call_video::{self, Host};
use super::props::{BoolProp, IntProp, UiProps};
use super::router;
use super::session::ActiveBackend;
use super::window_events::install_call_window_events;
use crate::app::input::CommandSender;

const MIRRORED_GLOBALS: [&str; 3] = ["CallView", "Motion", "Probe"];

type Opened = <ActiveBackend as UiBackend>::CallWindow;

type TileSeen = Box<dyn Fn(&str, i32, i32, bool)>;

pub struct WindowHooks {
    pub call_tile_seen: TileSeen,
    pub call_media_shown: Box<dyn Fn(bool)>,
    pub watch_microphone: Box<dyn Fn(bool)>,
    pub watch_connections: Box<dyn Fn(bool)>,
    pub show_call_window: Box<dyn Fn()>,
    pub close_call_window: Box<dyn Fn()>,
    pub show_call_chat: Box<dyn Fn()>,
    pub show_main_window: Box<dyn Fn()>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Watch {
    Microphone,
    Connections,
}

#[derive(Clone, Copy)]
enum Raised {
    Main,
    Call,
}

thread_local! {
    static OPEN: RefCell<Option<Opened>> = const { RefCell::new(None) };
    static SHOWN_CALL: Cell<i32> = const { Cell::new(0) };
    static COMMANDS: RefCell<Option<CommandSender>> = const { RefCell::new(None) };
    static WATCHES: RefCell<HashSet<(Host, Watch)>> = RefCell::new(HashSet::new());
    static RAISING: RefCell<Vec<(AsyncRequestSerial, Raised)>> = const { RefCell::new(Vec::new()) };
}

pub fn install_commands(commands: &CommandSender) {
    COMMANDS.set(Some(commands.clone()));
}

pub fn commands() -> Option<CommandSender> {
    COMMANDS.with_borrow(Clone::clone)
}

pub fn mirrors(global: &str) -> bool {
    MIRRORED_GLOBALS.contains(&global)
}

pub fn mirror<T: Clone>(value: &T, write: impl FnOnce(&Opened, T)) {
    OPEN.with_borrow(|open| {
        if let Some(window) = open {
            write(window, value.clone());
        }
    });
}

pub fn opened() -> Option<Opened> {
    OPEN.with_borrow(|open| open.as_ref().map(ComponentHandle::clone_strong))
}

pub fn keep(window: Option<Opened>) -> Option<Opened> {
    OPEN.replace(window)
}

pub fn hooks<B: UiBackend>(main: &B::Window, host: Host) -> WindowHooks {
    let seeing = main.as_weak();
    let showing = main.as_weak();
    let opening = main.as_weak();
    let closing = main.as_weak();
    let chatting = main.as_weak();
    let raising = main.as_weak();
    WindowHooks {
        call_tile_seen: Box::new(move |feed, width, height, shown| {
            if let Some(main) = seeing.upgrade()
                && reports::<B>(host)
            {
                call_video::seen::<B>(&main, host, feed, width, height, shown);
            }
        }),
        call_media_shown: Box::new(move |on| {
            if let Some(main) = showing.upgrade()
                && reports::<B>(host)
            {
                call_video::media_shown::<B>(&main, host, on);
            }
        }),
        watch_microphone: Box::new(move |on| {
            if reports::<B>(host) {
                watch(host, Watch::Microphone, on);
            }
        }),
        watch_connections: Box::new(move |on| {
            if reports::<B>(host) {
                watch(host, Watch::Connections, on);
            }
        }),
        show_call_window: Box::new(move || {
            if let Some(main) = opening.upgrade() {
                show::<B>(&main);
            }
        }),
        close_call_window: Box::new(move || {
            if let Some(main) = closing.upgrade() {
                close::<B>(&main);
            }
        }),
        show_call_chat: Box::new(move || {
            if let Some(main) = chatting.upgrade() {
                show_call_chat::<B>(&main);
            }
        }),
        show_main_window: Box::new(move || {
            if let Some(main) = raising.upgrade() {
                raise_main::<B>(&main);
            }
        }),
    }
}

fn reports<B: UiBackend>(host: Host) -> bool {
    host == Host::Main || B::call_window().is_some()
}

pub fn show<B: UiBackend>(main: &B::Window) {
    if B::call_window().is_some() {
        raise_other::<B>(main.window(), Raised::Call, main);
        return;
    }
    let call = main.get_int(IntProp::ActiveCall);
    if call == 0 {
        return;
    }
    let Some(window) = B::new_call_window(main, hooks::<B>(main, Host::CallWindow)) else {
        return;
    };
    install_call_window_events::<B>(&window, main);
    let closing = main.as_weak();
    window.window().on_close_requested(move || {
        if let Some(main) = closing.upgrade() {
            close::<B>(&main);
        }
        CloseRequestResponse::HideWindow
    });
    SHOWN_CALL.set(call);
    drop(B::keep_call_window(Some(window.clone_strong())));
    if let Err(e) = window.show() {
        tracing::warn!("the call window could not open: {e}");
        close::<B>(main);
        return;
    }
    tracing::debug!(call, "the call moved to its own window");
    main.set_bool(BoolProp::CallWindowed, true);
    main.window().request_redraw();
    raise_other::<B>(main.window(), Raised::Call, main);
}

pub fn close<B: UiBackend>(main: &B::Window) {
    let Some(window) = B::keep_call_window(None) else {
        return;
    };
    tracing::debug!("the call came back to the main window");
    retire(window);
    SHOWN_CALL.set(0);
    call_video::forget::<B>(main, Host::CallWindow);
    forget_watches(Host::CallWindow);
    main.set_bool(BoolProp::CallWindowed, false);
    main.window().request_redraw();
}

pub fn dismiss<B: UiBackend>() {
    if let Some(window) = B::keep_call_window(None)
        && let Err(e) = window.hide()
    {
        tracing::warn!("the call window could not be hidden: {e}");
    }
}

pub fn follow<B: UiBackend>(main: &B::Window) {
    let shown = SHOWN_CALL.get();
    if shown != 0 && main.get_int(IntProp::ActiveCall) != shown {
        close::<B>(main);
    }
}

#[cfg(feature = "demo")]
pub fn request_close<B: UiBackend>() {
    if let Some(window) = B::call_window() {
        window
            .window()
            .dispatch_event(PlatformEvent::CloseRequested);
    }
}

pub fn take_token<B: UiBackend>(event: &WindowEvent, main: &slint::Weak<B::Window>) {
    let WindowEvent::ActivationTokenDone { serial, token } = event else {
        return;
    };
    let target = RAISING.with_borrow_mut(|raising| {
        let at = raising.iter().position(|(asked, _)| asked == serial)?;
        Some(raising.remove(at).1)
    });
    if let (Some(target), Some(main)) = (target, main.upgrade()) {
        raise_now::<B>(target, Some(&token.clone().into_raw()), &main);
    }
}

fn show_call_chat<B: UiBackend>(main: &B::Window) {
    raise_main::<B>(main);
    let asks = main.get_int(IntProp::CallChatAsks).wrapping_add(1);
    main.set_int(IntProp::CallChatAsks, asks);
    main.window().request_redraw();
}

fn raise_main<B: UiBackend>(main: &B::Window) {
    match B::call_window() {
        Some(window) => raise_other::<B>(window.window(), Raised::Main, main),
        None => raise(main.window(), None),
    }
}

fn raise_other<B: UiBackend>(asking: &Window, target: Raised, main: &B::Window) {
    match request_token(asking) {
        Some(serial) => RAISING.with_borrow_mut(|raising| raising.push((serial, target))),
        None => raise_now::<B>(target, None, main),
    }
}

fn raise_now<B: UiBackend>(target: Raised, token: Option<&str>, main: &B::Window) {
    match target {
        Raised::Main => raise(main.window(), token),
        Raised::Call => {
            if let Some(window) = B::call_window() {
                raise(window.window(), token);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn request_token(window: &Window) -> Option<AsyncRequestSerial> {
    window
        .with_winit_window(|winit| winit.request_activation_token().ok())
        .flatten()
}

#[cfg(not(target_os = "linux"))]
fn request_token(_window: &Window) -> Option<AsyncRequestSerial> {
    None
}

fn retire(window: impl ComponentHandle + 'static) {
    if let Err(e) = window.hide() {
        tracing::warn!("the call window could not be hidden: {e}");
    }
    Timer::single_shot(Duration::ZERO, move || drop(window));
}

fn watch(host: Host, watch: Watch, on: bool) {
    let (before, after) = WATCHES.with_borrow_mut(|watches| {
        let before = watched(watches, watch);
        if on {
            watches.insert((host, watch));
        } else {
            watches.remove(&(host, watch));
        }
        (before, watched(watches, watch))
    });
    if before != after {
        send_watch(watch, after);
    }
}

fn forget_watches(host: Host) {
    for kind in [Watch::Microphone, Watch::Connections] {
        watch(host, kind, false);
    }
}

fn watched(watches: &HashSet<(Host, Watch)>, watch: Watch) -> bool {
    watches.iter().any(|(_, kind)| *kind == watch)
}

fn send_watch(watch: Watch, on: bool) {
    COMMANDS.with_borrow(|commands| {
        let Some(commands) = commands else {
            return;
        };
        match watch {
            Watch::Microphone => router::watch_microphone(commands, on),
            Watch::Connections => router::watch_connections(commands, on),
        }
    });
}
