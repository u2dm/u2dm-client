#[cfg(target_os = "linux")]
use std::cell::RefCell;
#[cfg(target_os = "linux")]
use std::ffi::c_void;
#[cfg(target_os = "linux")]
use std::mem::ManuallyDrop;

use slint::Window;
use slint::winit_030::WinitWindowAccessor;
#[cfg(target_os = "linux")]
use slint::winit_030::winit::raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use slint::winit_030::winit::window::UserAttentionType;
#[cfg(target_os = "linux")]
use wayland_client::backend::{Backend, ObjectId};
#[cfg(target_os = "linux")]
use wayland_client::protocol::wl_registry::{self, WlRegistry};
#[cfg(target_os = "linux")]
use wayland_client::protocol::wl_surface::WlSurface;
#[cfg(target_os = "linux")]
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
#[cfg(target_os = "linux")]
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::{self, XdgActivationV1};

#[cfg(target_os = "linux")]
const ACTIVATION_VERSION: u32 = 1;

#[cfg(target_os = "linux")]
thread_local! {
    static GUEST: RefCell<Guest> = const { RefCell::new(Guest::Unopened) };
}

#[cfg(target_os = "linux")]
enum Guest {
    Unopened,
    Activating(ManuallyDrop<Box<Activator>>),
    Silent,
}

#[cfg(target_os = "linux")]
struct Activator {
    connection: Connection,
    queue: EventQueue<Activation>,
    state: Activation,
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct Activation {
    global: Option<XdgActivationV1>,
}

pub fn raise(window: &Window, token: Option<&str>) {
    window.set_minimized(false);
    if token.is_some_and(|token| activate_with_token(window, token)) {
        tracing::debug!("raised the window with an activation token");
        return;
    }
    window.with_winit_window(|winit| {
        winit.focus_window();
        if !winit.has_focus() {
            winit.request_user_attention(Some(UserAttentionType::Informational));
        }
    });
}

#[cfg(target_os = "linux")]
fn activate_with_token(window: &Window, token: &str) -> bool {
    let Some((display, surface)) = wayland_handles(window) else {
        return false;
    };
    GUEST.with_borrow_mut(|guest| {
        if matches!(guest, Guest::Unopened) {
            *guest = open(display);
        }
        match guest {
            Guest::Activating(activator) => activator.activate(surface, token),
            Guest::Unopened | Guest::Silent => false,
        }
    })
}

#[cfg(not(target_os = "linux"))]
fn activate_with_token(_window: &Window, _token: &str) -> bool {
    false
}

#[cfg(target_os = "linux")]
impl Activator {
    fn activate(&mut self, surface: *mut c_void, token: &str) -> bool {
        drop(self.queue.dispatch_pending(&mut self.state));
        let Some(global) = self.state.global.as_ref() else {
            return false;
        };
        let surface = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.cast()) }
            .ok()
            .and_then(|id| WlSurface::from_id(&self.connection, id).ok());
        let Some(surface) = surface else {
            tracing::debug!("the window's Wayland surface could not be named");
            return false;
        };
        global.activate(token.to_owned(), &surface);
        self.connection.flush().is_ok()
    }
}

#[cfg(target_os = "linux")]
fn open(display: *mut c_void) -> Guest {
    let connection =
        Connection::from_backend(unsafe { Backend::from_foreign_display(display.cast()) });
    let mut queue = connection.new_event_queue();
    let mut state = Activation::default();
    drop(connection.display().get_registry(&queue.handle(), ()));
    if queue.roundtrip(&mut state).is_err() || state.global.is_none() {
        tracing::debug!("the compositor offers no window activation");
        return Guest::Silent;
    }
    Guest::Activating(ManuallyDrop::new(Box::new(Activator {
        connection,
        queue,
        state,
    })))
}

#[cfg(target_os = "linux")]
fn wayland_handles(window: &Window) -> Option<(*mut c_void, *mut c_void)> {
    window.with_winit_window(|winit| {
        let display = match winit.display_handle().ok()?.as_raw() {
            RawDisplayHandle::Wayland(wayland) => wayland.display.as_ptr(),
            _ => return None,
        };
        let surface = match winit.window_handle().ok()?.as_raw() {
            RawWindowHandle::Wayland(wayland) => wayland.surface.as_ptr(),
            _ => return None,
        };
        Some((display, surface))
    })?
}

#[cfg(target_os = "linux")]
impl Dispatch<WlRegistry, ()> for Activation {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
            && interface == XdgActivationV1::interface().name
            && state.global.is_none()
        {
            state.global =
                Some(registry.bind::<XdgActivationV1, _, _>(name, ACTIVATION_VERSION, queue, ()));
        }
    }
}

#[cfg(target_os = "linux")]
impl Dispatch<XdgActivationV1, ()> for Activation {
    fn event(
        _: &mut Self,
        _: &XdgActivationV1,
        _: xdg_activation_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
