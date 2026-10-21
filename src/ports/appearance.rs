use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionPreference {
    Full,
    Reduced,
}

pub type MotionSink = Arc<dyn Fn(MotionPreference) + Send + Sync>;

pub trait AppearancePort: Send + Sync {
    fn follow_motion(&self, sink: MotionSink);
}
