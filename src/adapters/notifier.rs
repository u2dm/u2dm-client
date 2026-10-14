#[cfg(target_os = "linux")]
mod freedesktop;

use std::sync::Arc;

#[cfg(not(target_os = "linux"))]
use crate::domain::room::RoomId;
use crate::ports::notifier::NotifierPort;
#[cfg(not(target_os = "linux"))]
use crate::ports::notifier::{ActivationSink, DesktopNotice};

#[cfg(target_os = "linux")]
pub fn desktop() -> Arc<dyn NotifierPort> {
    Arc::new(freedesktop::FreedesktopNotifier::new())
}

#[cfg(not(target_os = "linux"))]
pub fn desktop() -> Arc<dyn NotifierPort> {
    Arc::new(SilentNotifier)
}

#[cfg(not(target_os = "linux"))]
struct SilentNotifier;

#[cfg(not(target_os = "linux"))]
impl NotifierPort for SilentNotifier {
    fn show(&self, notice: DesktopNotice) {
        tracing::debug!(room_id = %notice.key, "this platform shows no desktop notifications");
    }

    fn withdraw(&self, _key: &RoomId) {}

    fn withdraw_all(&self) {}

    fn on_activation(&self, _sink: ActivationSink) {}
}
