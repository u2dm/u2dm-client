#[cfg(target_os = "linux")]
use std::cell::RefCell;
#[cfg(target_os = "linux")]
use std::ffi::c_void;
#[cfg(target_os = "linux")]
use std::mem::ManuallyDrop;
#[cfg(target_os = "linux")]
use std::time::Duration;

use slint::ComponentHandle;
#[cfg(target_os = "linux")]
use slint::winit_030::WinitWindowAccessor;
#[cfg(target_os = "linux")]
use slint::winit_030::winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
#[cfg(target_os = "linux")]
use slint::{Timer, TimerMode, Weak, Window};
#[cfg(target_os = "linux")]
use wayland_client::backend::Backend;
#[cfg(target_os = "linux")]
use wayland_client::protocol::wl_pointer::{self, WlPointer};
#[cfg(target_os = "linux")]
use wayland_client::protocol::wl_registry::{self, WlRegistry};
#[cfg(target_os = "linux")]
use wayland_client::protocol::wl_seat::{self, Capability, WlSeat};
#[cfg(target_os = "linux")]
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
#[cfg(target_os = "linux")]
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gesture_pinch_v1::{
    self, ZwpPointerGesturePinchV1,
};
#[cfg(target_os = "linux")]
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gestures_v1::{
    self, ZwpPointerGesturesV1,
};

use super::backend::UiBackend;
use super::props::{IntProp, UiProps};

#[cfg(target_os = "linux")]
const GESTURES_VERSION: u32 = 1;
#[cfg(target_os = "linux")]
const POINTER_RELEASE_SINCE: u32 = 3;
#[cfg(target_os = "linux")]
const PINCH_POLL: Duration = Duration::from_millis(16);
#[cfg(target_os = "linux")]
const PERMILLE: f64 = 1000.0;

#[cfg(target_os = "linux")]
thread_local! {
    static WATCH: RefCell<Watch> = const { RefCell::new(Watch::Unopened) };
}

#[cfg(target_os = "linux")]
enum Watch {
    Unopened,
    Reading(ManuallyDrop<Box<Reader>>),
    Silent,
}

#[cfg(target_os = "linux")]
struct Reader {
    connection: Connection,
    queue: EventQueue<Pinches>,
    pinches: Pinches,
    poll: Timer,
    published: i32,
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct Pinches {
    gestures: Option<ZwpPointerGesturesV1>,
    seats: Vec<WlSeat>,
    followed: Vec<(WlPointer, ZwpPointerGesturePinchV1)>,
    following: bool,
    scale: Option<f64>,
}

#[cfg(target_os = "linux")]
pub fn follow<B: UiBackend>(window: &B::Window, videos_shown: bool) {
    let ended = WATCH.with_borrow_mut(|watch| {
        if videos_shown && matches!(watch, Watch::Unopened) {
            *watch = wayland_display(window.window()).map_or(Watch::Silent, open);
        }
        match watch {
            Watch::Reading(reader) if videos_shown => {
                reader.start::<B>(window.as_weak());
                false
            }
            Watch::Reading(reader) => reader.stop(),
            Watch::Unopened | Watch::Silent => false,
        }
    });
    if ended {
        publish::<B>(window, 0);
    }
}

#[cfg(not(target_os = "linux"))]
pub fn follow<B: UiBackend>(_window: &B::Window, _videos_shown: bool) {}

pub fn publish<B: UiBackend>(window: &B::Window, permille: i32) {
    window.set_int(IntProp::TouchpadPinch, permille);
    window.window().request_redraw();
    if let Some(call) = B::call_window() {
        call.window().request_redraw();
    }
}

#[cfg(target_os = "linux")]
fn take_pinch<B: UiBackend>(weak: &Weak<B::Window>) {
    let changed = WATCH.with_borrow_mut(|watch| match watch {
        Watch::Reading(reader) => reader.take_events(),
        Watch::Unopened | Watch::Silent => None,
    });
    if let (Some(permille), Some(window)) = (changed, weak.upgrade()) {
        publish::<B>(&window, permille);
    }
}

#[cfg(target_os = "linux")]
impl Reader {
    fn start<B: UiBackend>(&mut self, weak: Weak<B::Window>) {
        if self.poll.running() {
            return;
        }
        self.pinches.follow(&self.queue.handle());
        drop(self.connection.flush());
        self.poll.start(TimerMode::Repeated, PINCH_POLL, move || {
            take_pinch::<B>(&weak);
        });
        tracing::debug!(
            pointers = self.pinches.followed.len(),
            "following touchpad pinches while call videos are shown"
        );
    }

    fn stop(&mut self) -> bool {
        if !self.poll.running() {
            return false;
        }
        self.poll.stop();
        self.pinches.unfollow();
        drop(self.connection.flush());
        tracing::debug!("no call videos shown: touchpad pinches are not followed");
        drop(self.queue.dispatch_pending(&mut self.pinches));
        self.pinches.scale = None;
        let ended = self.published != 0;
        self.published = 0;
        ended
    }

    fn take_events(&mut self) -> Option<i32> {
        if self.queue.dispatch_pending(&mut self.pinches).is_err() {
            drop(self.connection.flush());
        }
        let permille = self.pinches.scale.map_or(0, in_permille);
        if permille == self.published {
            return None;
        }
        self.published = permille;
        Some(permille)
    }
}

#[cfg(target_os = "linux")]
impl Pinches {
    fn follow(&mut self, queue: &QueueHandle<Self>) {
        self.following = true;
        if let Some(gestures) = &self.gestures {
            self.followed.extend(
                self.seats
                    .iter()
                    .map(|seat| pinch_of(gestures, seat, queue)),
            );
        }
    }

    fn unfollow(&mut self) {
        self.following = false;
        for (pointer, pinch) in self.followed.drain(..) {
            pinch.destroy();
            pointer.release();
        }
    }
}

#[cfg(target_os = "linux")]
fn pinch_of(
    gestures: &ZwpPointerGesturesV1,
    seat: &WlSeat,
    queue: &QueueHandle<Pinches>,
) -> (WlPointer, ZwpPointerGesturePinchV1) {
    let pointer = seat.get_pointer(queue, ());
    let pinch = gestures.get_pinch_gesture(&pointer, queue, ());
    (pointer, pinch)
}

#[cfg(target_os = "linux")]
#[allow(clippy::cast_possible_truncation)]
fn in_permille(scale: f64) -> i32 {
    (scale * PERMILLE).round().clamp(1.0, f64::from(i32::MAX)) as i32
}

#[cfg(target_os = "linux")]
fn open(display: *mut c_void) -> Watch {
    let connection =
        Connection::from_backend(unsafe { Backend::from_foreign_display(display.cast()) });
    let mut queue = connection.new_event_queue();
    let mut pinches = Pinches::default();
    drop(connection.display().get_registry(&queue.handle(), ()));
    for _ in 0..2 {
        if queue.roundtrip(&mut pinches).is_err() {
            return Watch::Silent;
        }
    }
    if pinches.gestures.is_none() || pinches.seats.is_empty() {
        tracing::debug!("the compositor reports no touchpad pinches");
        return Watch::Silent;
    }
    tracing::debug!(seats = pinches.seats.len(), "watching touchpad pinches");
    Watch::Reading(ManuallyDrop::new(Box::new(Reader {
        connection,
        queue,
        pinches,
        poll: Timer::default(),
        published: 0,
    })))
}

#[cfg(target_os = "linux")]
fn wayland_display(window: &Window) -> Option<*mut c_void> {
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

#[cfg(target_os = "linux")]
impl Dispatch<WlRegistry, ()> for Pinches {
    fn event(
        pinches: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        if interface == WlSeat::interface().name && version >= POINTER_RELEASE_SINCE {
            drop(registry.bind::<WlSeat, _, _>(name, POINTER_RELEASE_SINCE, queue, ()));
        } else if interface == ZwpPointerGesturesV1::interface().name && pinches.gestures.is_none()
        {
            pinches.gestures = Some(registry.bind::<ZwpPointerGesturesV1, _, _>(
                name,
                GESTURES_VERSION,
                queue,
                (),
            ));
        }
    }
}

#[cfg(target_os = "linux")]
impl Dispatch<WlSeat, ()> for Pinches {
    fn event(
        pinches: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        (): &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
            && capabilities.contains(Capability::Pointer)
            && !pinches.seats.contains(seat)
        {
            pinches.seats.push(seat.clone());
            if pinches.following
                && let Some(gestures) = &pinches.gestures
            {
                pinches.followed.push(pinch_of(gestures, seat, queue));
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl Dispatch<WlPointer, ()> for Pinches {
    fn event(
        _: &mut Self,
        _: &WlPointer,
        _: wl_pointer::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[cfg(target_os = "linux")]
impl Dispatch<ZwpPointerGesturesV1, ()> for Pinches {
    fn event(
        _: &mut Self,
        _: &ZwpPointerGesturesV1,
        _: zwp_pointer_gestures_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[cfg(target_os = "linux")]
impl Dispatch<ZwpPointerGesturePinchV1, ()> for Pinches {
    fn event(
        pinches: &mut Self,
        _: &ZwpPointerGesturePinchV1,
        event: zwp_pointer_gesture_pinch_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_pointer_gesture_pinch_v1::Event::Begin { fingers, .. } => {
                tracing::debug!(fingers, "a touchpad pinch began");
                pinches.scale = Some(1.0);
            }
            zwp_pointer_gesture_pinch_v1::Event::Update { scale, .. }
                if pinches.scale.is_some() =>
            {
                pinches.scale = Some(scale);
            }
            zwp_pointer_gesture_pinch_v1::Event::End { cancelled, .. } => {
                tracing::debug!(
                    scale = ?pinches.scale,
                    cancelled = cancelled != 0,
                    "a touchpad pinch ended"
                );
                pinches.scale = None;
            }
            _ => {}
        }
    }
}
