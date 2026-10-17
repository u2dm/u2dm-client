use std::sync::Arc;

#[cfg(feature = "calls")]
use crate::adapters::livekit::LiveKitMedia;
use crate::ports::call::CallMediaPort;

#[cfg(feature = "calls")]
#[allow(clippy::unnecessary_wraps)]
pub fn production() -> Option<Arc<dyn CallMediaPort>> {
    Some(Arc::new(LiveKitMedia::new()))
}

#[cfg(not(feature = "calls"))]
pub const fn production() -> Option<Arc<dyn CallMediaPort>> {
    None
}
