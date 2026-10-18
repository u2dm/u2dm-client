use std::sync::Arc;

#[cfg(feature = "calls")]
use crate::adapters::livekit::LiveKitMedia;
use crate::ports::call::{CallMediaPort, CallVideoPort};

pub struct CallPorts {
    pub media: Option<Arc<dyn CallMediaPort>>,
    pub video: Option<Arc<dyn CallVideoPort>>,
}

#[cfg(feature = "calls")]
pub fn production() -> CallPorts {
    let livekit = Arc::new(LiveKitMedia::new());
    let media: Arc<dyn CallMediaPort> = Arc::<LiveKitMedia>::clone(&livekit);
    CallPorts {
        media: Some(media),
        video: Some(livekit),
    }
}

#[cfg(not(feature = "calls"))]
pub const fn production() -> CallPorts {
    CallPorts {
        media: None,
        video: None,
    }
}
