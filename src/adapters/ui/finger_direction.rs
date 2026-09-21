use std::cell::Cell;
#[cfg(target_os = "linux")]
use std::cell::RefCell;
#[cfg(target_os = "linux")]
use std::ffi::c_void;
#[cfg(target_os = "linux")]
use std::mem::ManuallyDrop;
use std::rc::Rc;

use slint::Window;
#[cfg(target_os = "linux")]
use slint::winit_030::WinitWindowAccessor;
#[cfg(target_os = "linux")]
use slint::winit_030::winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
#[cfg(target_os = "linux")]
use wayland_client::backend::{Backend, ObjectId};
#[cfg(target_os = "linux")]
use wayland_client::protocol::wl_pointer::{self, Axis, AxisRelativeDirection, WlPointer};
#[cfg(target_os = "linux")]
use wayland_client::protocol::wl_registry::{self, WlRegistry};
#[cfg(target_os = "linux")]
use wayland_client::protocol::wl_seat::{self, Capability, WlSeat};
#[cfg(target_os = "linux")]
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};

#[cfg(target_os = "linux")]
const AXIS_DIRECTION_SINCE: u32 = 9;

pub struct FingerDirection {
    inverted: Rc<Cell<bool>>,
    #[cfg(target_os = "linux")]
    watch: RefCell<Watch>,
}

impl FingerDirection {
    pub fn new() -> Self {
        Self {
            inverted: Rc::new(Cell::new(true)),
            #[cfg(target_os = "linux")]
            watch: RefCell::new(Watch::Unopened),
        }
    }

    pub fn leftward(&self, delta_x: f32) -> f32 {
        if self.inverted.get() {
            -delta_x
        } else {
            delta_x
        }
    }

    #[cfg(target_os = "linux")]
    pub fn follow(&self, window: &Window) {
        let mut watch = self.watch.borrow_mut();
        match &mut *watch {
            Watch::Unopened => *watch = open(window, &self.inverted),
            Watch::Reporting(pointers) => pointers.take_events(),
            Watch::Silent => {}
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn follow(&self, _window: &Window) {}
}

#[cfg(target_os = "linux")]
enum Watch {
    Unopened,
    Reporting(ManuallyDrop<Box<Reporting>>),
    Silent,
}

#[cfg(target_os = "linux")]
struct Reporting {
    queue: EventQueue<Pointers>,
    pointers: Pointers,
    connection: Connection,
}

#[cfg(target_os = "linux")]
impl Reporting {
    fn take_events(&mut self) {
        if self.queue.dispatch_pending(&mut self.pointers).is_err() {
            drop(self.connection.flush());
        }
    }
}

#[cfg(target_os = "linux")]
struct Pointers {
    inverted: Rc<Cell<bool>>,
    seated: Vec<ObjectId>,
}

#[cfg(target_os = "linux")]
fn open(window: &Window, inverted: &Rc<Cell<bool>>) -> Watch {
    let Some(display) = wayland_display(window) else {
        return Watch::Silent;
    };
    let connection =
        Connection::from_backend(unsafe { Backend::from_foreign_display(display.cast()) });
    let mut queue = connection.new_event_queue();
    let mut pointers = Pointers {
        inverted: Rc::clone(inverted),
        seated: Vec::new(),
    };
    drop(connection.display().get_registry(&queue.handle(), ()));
    for _ in 0..2 {
        if queue.roundtrip(&mut pointers).is_err() {
            return Watch::Silent;
        }
    }
    if pointers.seated.is_empty() {
        tracing::debug!("the compositor does not report the physical scroll direction");
        return Watch::Silent;
    }
    tracing::debug!(
        seats = pointers.seated.len(),
        "watching the physical scroll direction"
    );
    Watch::Reporting(ManuallyDrop::new(Box::new(Reporting {
        queue,
        pointers,
        connection,
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
impl Dispatch<WlRegistry, ()> for Pointers {
    fn event(
        _: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == WlSeat::interface().name
            && version >= AXIS_DIRECTION_SINCE
        {
            drop(registry.bind::<WlSeat, _, _>(name, AXIS_DIRECTION_SINCE, queue, ()));
        }
    }
}

#[cfg(target_os = "linux")]
impl Dispatch<WlSeat, ()> for Pointers {
    fn event(
        pointers: &mut Self,
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
            && !pointers.seated.contains(&seat.id())
        {
            pointers.seated.push(seat.id());
            drop(seat.get_pointer(queue, ()));
        }
    }
}

#[cfg(target_os = "linux")]
impl Dispatch<WlPointer, ()> for Pointers {
    fn event(
        pointers: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::AxisRelativeDirection {
            axis: WEnum::Value(Axis::HorizontalScroll),
            direction: WEnum::Value(direction),
        } = event
        {
            let inverted = direction == AxisRelativeDirection::Inverted;
            if pointers.inverted.replace(inverted) != inverted {
                tracing::debug!(inverted, "the physical scroll direction changed");
            }
        }
    }
}
