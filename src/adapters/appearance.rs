#[cfg(target_os = "linux")]
mod portal;

use std::sync::Arc;

use crate::ports::appearance::AppearancePort;
#[cfg(not(target_os = "linux"))]
use crate::ports::appearance::MotionSink;

#[cfg(target_os = "linux")]
pub fn desktop() -> Arc<dyn AppearancePort> {
    Arc::new(portal::PortalAppearance)
}

#[cfg(not(target_os = "linux"))]
pub fn desktop() -> Arc<dyn AppearancePort> {
    Arc::new(SilentAppearance)
}

#[cfg(not(target_os = "linux"))]
struct SilentAppearance;

#[cfg(not(target_os = "linux"))]
impl AppearancePort for SilentAppearance {
    fn follow_motion(&self, _sink: MotionSink) {
        tracing::debug!("this platform reports no motion preference, so every animation runs");
    }
}
