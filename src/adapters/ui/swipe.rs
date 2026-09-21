use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use slint::platform::WindowEvent as PointerEvent;
use slint::winit_030::EventResult;
use slint::winit_030::winit::dpi::PhysicalPosition;
use slint::winit_030::winit::event::{MouseScrollDelta, TouchPhase, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Timer, TimerMode, Weak, Window};

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
    Measuring { x: f32, y: f32 },
    Replying { travel: f32 },
    Scrolling,
}

pub fn reply_swipe<B: UiBackend>(
    window: &B::Window,
) -> impl FnMut(&Window, &WindowEvent) -> EventResult + use<B> {
    let swipe = Swipe::<B> {
        weak: window.as_weak(),
        cursor: Cell::default(),
        gesture: Rc::new(Cell::new(Gesture::Idle)),
        idle: Timer::default(),
        fingers: FingerDirection::new(),
    };
    move |window, event| swipe.handle(window, event)
}

struct Swipe<B: UiBackend> {
    weak: Weak<B::Window>,
    cursor: Cell<LogicalPosition>,
    gesture: Rc<Cell<Gesture>>,
    idle: Timer,
    fingers: FingerDirection,
}

impl<B: UiBackend> Swipe<B> {
    fn handle(&self, window: &Window, event: &WindowEvent) -> EventResult {
        self.fingers.follow(window);
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor.set(logical(window, *position));
                EventResult::Propagate
            }
            WindowEvent::MouseWheel { delta, phase, .. } => self.wheel(window, *delta, *phase),
            _ => EventResult::Propagate,
        }
    }

    fn wheel(&self, window: &Window, delta: MouseScrollDelta, phase: TouchPhase) -> EventResult {
        let MouseScrollDelta::PixelDelta(pixels) = delta else {
            self.finish();
            return EventResult::Propagate;
        };
        if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
            return if self.finish() {
                EventResult::PreventDefault
            } else {
                EventResult::Propagate
            };
        }

        self.wait_for_the_fingers_to_lift();
        let step = logical(window, pixels);
        if matches!(phase, TouchPhase::Started) || matches!(self.gesture.get(), Gesture::Idle) {
            self.gesture.set(self.open());
        }

        match self.gesture.get() {
            Gesture::Measuring { x, y } => self.measure(window, x + step.x, y + step.y),
            Gesture::Replying { travel } => self.pull(travel + self.fingers.leftward(step.x)),
            Gesture::Idle | Gesture::Scrolling => EventResult::Propagate,
        }
    }

    fn open(&self) -> Gesture {
        if self.armed() {
            Gesture::Measuring { x: 0.0, y: 0.0 }
        } else {
            Gesture::Scrolling
        }
    }

    fn measure(&self, window: &Window, x: f32, y: f32) -> EventResult {
        if x.abs().max(y.abs()) < AXIS_LOCK {
            self.gesture.set(Gesture::Measuring { x, y });
            return EventResult::PreventDefault;
        }
        let leftward = self.fingers.leftward(x);
        if leftward > y.abs() * AXIS_DOMINANCE {
            return self.pull(leftward);
        }
        self.gesture.set(Gesture::Scrolling);
        window.dispatch_event(PointerEvent::PointerScrolled {
            position: self.cursor.get(),
            delta_x: x,
            delta_y: y,
        });
        EventResult::PreventDefault
    }

    fn pull(&self, travel: f32) -> EventResult {
        let travel = travel.max(0.0);
        self.gesture.set(Gesture::Replying { travel });
        self.publish(travel);
        EventResult::PreventDefault
    }

    fn finish(&self) -> bool {
        self.idle.stop();
        let pulled = matches!(self.gesture.get(), Gesture::Replying { .. });
        if pulled {
            self.publish(0.0);
        }
        self.gesture.set(Gesture::Idle);
        pulled
    }

    fn wait_for_the_fingers_to_lift(&self) {
        if self.idle.running() {
            self.idle.restart();
            return;
        }
        let gesture = Rc::clone(&self.gesture);
        let weak = self.weak.clone();
        self.idle
            .start(TimerMode::SingleShot, GESTURE_GAP, move || {
                if matches!(gesture.get(), Gesture::Replying { .. })
                    && let Some(window) = weak.upgrade()
                {
                    window.set_int(IntProp::SwipeTravel, 0);
                }
                gesture.set(Gesture::Idle);
            });
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

fn logical(window: &Window, position: PhysicalPosition<f64>) -> LogicalPosition {
    let position = position.to_logical::<f32>(f64::from(window.scale_factor()));
    LogicalPosition::new(position.x, position.y)
}

#[allow(clippy::cast_possible_truncation)]
fn travel_px(travel: f32) -> i32 {
    travel.clamp(0.0, TRAVEL_CAP).round() as i32
}
