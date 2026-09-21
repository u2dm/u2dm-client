use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use slint::platform::WindowEvent as SlintEvent;
use slint::winit_030::EventResult;
use slint::winit_030::winit::dpi::{LogicalPosition, PhysicalPosition};
use slint::winit_030::winit::event::{MouseScrollDelta, TouchPhase, WindowEvent};
use slint::{ComponentHandle, LogicalPosition as SlintPosition, Timer, TimerMode, Weak, Window};

use super::backend::UiBackend;
use super::finger_direction::FingerDirection;
use super::props::{BoolProp, IntProp, UiProps};

const AXIS_LOCK: f32 = 8.0;
const AXIS_DOMINANCE: f32 = 1.5;
const GESTURE_GAP: Duration = Duration::from_millis(120);
const TRAVEL_CAP: f32 = 9999.0;

#[derive(Clone, Copy)]
enum Gesture {
    Idle,
    Unarmed,
    Measuring { x: f32, y: f32 },
    Replying { travel: f32 },
    Scrolling,
}

impl Gesture {
    const fn opened_over_a_row(self) -> bool {
        matches!(
            self,
            Self::Measuring { .. } | Self::Replying { .. } | Self::Scrolling
        )
    }
}

pub fn reply_swipe<B: UiBackend>(
    window: &B::Window,
) -> impl FnMut(&Window, &WindowEvent) -> EventResult + use<B> {
    let swipe = Swipe::<B> {
        state: Rc::new(State {
            weak: window.as_weak(),
            gesture: Cell::new(Gesture::Idle),
            pointer: Cell::new(None),
        }),
        idle: Timer::default(),
        fingers: FingerDirection::new(),
    };
    move |window, event| swipe.handle(window, event)
}

struct Swipe<B: UiBackend> {
    state: Rc<State<B>>,
    idle: Timer,
    fingers: FingerDirection,
}

struct State<B: UiBackend> {
    weak: Weak<B::Window>,
    gesture: Cell<Gesture>,
    pointer: Cell<Option<SlintPosition>>,
}

impl<B: UiBackend> Swipe<B> {
    fn handle(&self, window: &Window, event: &WindowEvent) -> EventResult {
        self.fingers.follow(window);
        match event {
            WindowEvent::MouseWheel { delta, phase, .. } => self.wheel(window, *delta, *phase),
            WindowEvent::CursorMoved { position, .. } => {
                let at = logical(window, *position);
                self.state.pointer.set(Some(SlintPosition::new(at.x, at.y)));
                EventResult::Propagate
            }
            WindowEvent::CursorLeft { .. } => {
                self.state.pointer.set(None);
                EventResult::Propagate
            }
            _ => EventResult::Propagate,
        }
    }

    fn wheel(&self, window: &Window, delta: MouseScrollDelta, phase: TouchPhase) -> EventResult {
        let MouseScrollDelta::PixelDelta(pixels) = delta else {
            self.finish();
            return EventResult::Propagate;
        };
        if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
            self.finish();
            return EventResult::Propagate;
        }

        self.wait_for_the_fingers_to_lift();
        let step = logical(window, pixels);
        if matches!(phase, TouchPhase::Started) || matches!(self.state.gesture.get(), Gesture::Idle)
        {
            self.state.gesture.set(self.state.open());
        }

        match self.state.gesture.get() {
            Gesture::Measuring { x, y } => self.measure(x + step.x, y + step.y),
            Gesture::Replying { travel } => self.state.pull(travel + self.fingers.leftward(step.x)),
            Gesture::Idle | Gesture::Unarmed | Gesture::Scrolling => EventResult::Propagate,
        }
    }

    fn measure(&self, x: f32, y: f32) -> EventResult {
        if x.abs().max(y.abs()) < AXIS_LOCK {
            self.state.gesture.set(Gesture::Measuring { x, y });
            return EventResult::Propagate;
        }
        let leftward = self.fingers.leftward(x);
        if leftward > y.abs() * AXIS_DOMINANCE {
            return self.state.claim(leftward);
        }
        self.state.gesture.set(Gesture::Scrolling);
        EventResult::Propagate
    }

    fn finish(&self) {
        self.idle.stop();
        self.state.end();
    }

    fn wait_for_the_fingers_to_lift(&self) {
        if self.idle.running() {
            self.idle.restart();
            return;
        }
        let state = Rc::clone(&self.state);
        self.idle
            .start(TimerMode::SingleShot, GESTURE_GAP, move || state.end());
    }
}

impl<B: UiBackend> State<B> {
    fn open(&self) -> Gesture {
        if self.armed() {
            Gesture::Measuring { x: 0.0, y: 0.0 }
        } else {
            Gesture::Unarmed
        }
    }

    fn claim(&self, travel: f32) -> EventResult {
        self.rehover();
        self.pull(travel)
    }

    fn pull(&self, travel: f32) -> EventResult {
        let travel = travel.max(0.0);
        self.gesture.set(Gesture::Replying { travel });
        self.publish(travel);
        EventResult::PreventDefault
    }

    fn end(self: &Rc<Self>) {
        let ended = self.gesture.replace(Gesture::Idle);
        if matches!(ended, Gesture::Replying { .. }) {
            self.publish(0.0);
        }
        if ended.opened_over_a_row() {
            let state = Rc::clone(self);
            Timer::single_shot(Duration::ZERO, move || state.rehover());
        }
    }

    fn rehover(&self) {
        if let Some(position) = self.pointer.get()
            && let Some(window) = self.weak.upgrade()
        {
            window
                .window()
                .dispatch_event(SlintEvent::PointerMoved { position });
        }
    }

    fn armed(&self) -> bool {
        self.weak
            .upgrade()
            .is_some_and(|window| window.get_bool(BoolProp::ReplySwipeArmed))
    }

    fn publish(&self, travel: f32) {
        if let Some(window) = self.weak.upgrade() {
            window.set_int(IntProp::SwipeTravel, travel_px(travel));
        }
    }
}

fn logical(window: &Window, physical: PhysicalPosition<f64>) -> LogicalPosition<f32> {
    physical.to_logical(f64::from(window.scale_factor()))
}

#[allow(clippy::cast_possible_truncation)]
fn travel_px(travel: f32) -> i32 {
    travel.clamp(0.0, TRAVEL_CAP).round() as i32
}
