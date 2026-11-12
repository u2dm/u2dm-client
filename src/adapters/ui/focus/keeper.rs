use std::cell::{Cell, RefCell};
use std::mem;
use std::rc::Rc;
use std::time::Duration;

use slint::platform::WindowEvent as SlintEvent;
use slint::private_unstable_api::re_exports::WindowInner;
use slint::winit_030::EventResult;
use slint::winit_030::winit::event::{ElementState, WindowEvent};
use slint::{ComponentHandle, RenderingState, SharedString, Timer, TimerMode, Window};

use super::keys::slint_text;

const RECOVERY_STEP: Duration = Duration::from_millis(16);
const TICKS_PER_LEVEL: u32 = 3;
const LAST_LEVEL: i32 = 4;

pub type Recover<C> = fn(&C, i32, bool) -> SharedString;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Episode {
    Settled,
    Recovering { eager: bool },
    WaitingForAKey,
    GaveUp,
}

pub struct FocusKeeper<C: ComponentHandle + 'static> {
    component: slint::Weak<C>,
    recover: Recover<C>,
    held: RefCell<Vec<SharedString>>,
    level: Cell<i32>,
    ticks: Cell<u32>,
    retry: Timer,
    episode: Cell<Episode>,
}

pub fn step_focus(window: &Window, forward: bool) {
    let inner = WindowInner::from_pub(window);
    if forward {
        inner.focus_next_item();
    } else {
        inner.focus_previous_item();
    }
}

fn holds_nothing(window: &Window) -> bool {
    WindowInner::from_pub(window)
        .focus_item
        .borrow()
        .upgrade()
        .is_none()
}

fn holds_nothing_in_sight(window: &Window) -> bool {
    WindowInner::from_pub(window)
        .focus_item
        .borrow()
        .upgrade()
        .is_none_or(|item| !item.is_visible())
}

impl<C: ComponentHandle + 'static> FocusKeeper<C> {
    pub fn install(component: &C, recover: Recover<C>) -> Rc<Self> {
        let keeper = Rc::new(Self {
            component: component.as_weak(),
            recover,
            held: RefCell::new(Vec::new()),
            level: Cell::new(0),
            ticks: Cell::new(0),
            retry: Timer::default(),
            episode: Cell::new(Episode::Settled),
        });
        let after_frames = Rc::downgrade(&keeper);
        let notified = component.window().set_rendering_notifier(move |state, _| {
            if matches!(state, RenderingState::AfterRendering)
                && let Some(keeper) = after_frames.upgrade()
            {
                keeper.after_frame();
            }
        });
        if let Err(e) = notified {
            tracing::debug!("focus that falls to nothing waits for a key, since {e}");
        }
        keeper
    }

    pub fn take(self: &Rc<Self>, slint_window: &Window, event: &WindowEvent) -> EventResult {
        let WindowEvent::KeyboardInput {
            event,
            is_synthetic: false,
            ..
        } = event
        else {
            return EventResult::Propagate;
        };
        if event.state != ElementState::Pressed {
            return EventResult::Propagate;
        }
        let Some(text) = slint_text(event) else {
            return EventResult::Propagate;
        };
        if self.retry.running() {
            self.held.borrow_mut().push(text);
            return EventResult::PreventDefault;
        }
        if !holds_nothing_in_sight(slint_window) {
            return EventResult::Propagate;
        }
        self.held.borrow_mut().push(text);
        self.begin(false);
        EventResult::PreventDefault
    }

    fn after_frame(self: &Rc<Self>) {
        let Some(component) = self.component.upgrade() else {
            return;
        };
        if !holds_nothing(component.window()) {
            if !self.retry.running() {
                self.episode.set(Episode::Settled);
            }
            return;
        }
        if self.episode.get() != Episode::Settled {
            return;
        }
        let keeper = Rc::downgrade(self);
        self.episode.set(Episode::Recovering { eager: true });
        Timer::single_shot(Duration::ZERO, move || {
            if let Some(keeper) = keeper.upgrade() {
                keeper.begin(true);
            }
        });
    }

    fn begin(self: &Rc<Self>, eager: bool) {
        let Some(component) = self.component.upgrade() else {
            return;
        };
        if eager && !holds_nothing(component.window()) {
            self.episode.set(Episode::Settled);
            return;
        }
        self.level.set(0);
        self.ticks.set(0);
        self.episode.set(Episode::Recovering { eager });
        if !self.recover_at(&component, 0, eager) {
            self.episode.set(Episode::WaitingForAKey);
            return;
        }
        let keeper = Rc::downgrade(self);
        self.retry
            .start(TimerMode::Repeated, RECOVERY_STEP, move || {
                if let Some(keeper) = keeper.upgrade() {
                    keeper.tick();
                }
            });
    }

    fn recover_at(&self, component: &C, level: i32, eager: bool) -> bool {
        let to = (self.recover)(component, level, eager);
        if to.is_empty() {
            tracing::debug!(
                level,
                eager,
                "focus fell to nothing; it waits for a key, since a list row held it"
            );
            return false;
        }
        tracing::debug!(to = %to, level, eager, "focus fell to nothing; recovered");
        true
    }

    fn tick(&self) {
        let Some(component) = self.component.upgrade() else {
            self.retry.stop();
            return;
        };
        if !holds_nothing_in_sight(component.window()) {
            self.settle(component.window());
            return;
        }
        let ticks = self.ticks.get() + 1;
        self.ticks.set(ticks);
        if !ticks.is_multiple_of(TICKS_PER_LEVEL) {
            return;
        }
        let level = self.level.get() + 1;
        if level > LAST_LEVEL {
            tracing::debug!("focus found no place to land; held keys go through as they are");
            self.settle(component.window());
            self.episode.set(Episode::GaveUp);
            return;
        }
        self.level.set(level);
        let eager = matches!(self.episode.get(), Episode::Recovering { eager: true });
        self.recover_at(&component, level, eager);
    }

    fn settle(&self, window: &Window) {
        self.retry.stop();
        self.episode.set(Episode::Settled);
        let held = mem::take(&mut *self.held.borrow_mut());
        for text in held {
            for event in [
                SlintEvent::KeyPressed { text: text.clone() },
                SlintEvent::KeyReleased { text },
            ] {
                if let Err(e) = window.dispatch_event_with_result(event) {
                    tracing::debug!("a held key could not be handed on: {e}");
                }
            }
        }
    }
}
