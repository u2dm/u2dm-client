use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use crate::adapters::handoff::DesktopHandoff;
use crate::ports::handoff::{Handoff, HandoffPort, HandoffSink};

pub const ENV_VAR: &str = "U2DM_DEMO_HANDOFF";

static HANDOFF: OnceLock<Arc<DemoHandoff>> = OnceLock::new();

struct DemoHandoff {
    sink: Mutex<Option<HandoffSink>>,
    socket: Option<DesktopHandoff>,
}

pub fn socket() -> Option<PathBuf> {
    env::var_os(ENV_VAR)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

pub fn port() -> Arc<dyn HandoffPort> {
    let handoff = HANDOFF.get_or_init(|| {
        Arc::new(DemoHandoff {
            sink: Mutex::new(None),
            socket: socket().map(DesktopHandoff::new),
        })
    });
    Arc::clone(handoff) as Arc<dyn HandoffPort>
}

pub fn hand_off(handoff: Handoff) -> bool {
    let sink = HANDOFF
        .get()
        .and_then(|demo| demo.sink.lock().ok().and_then(|sink| sink.clone()));
    let Some(sink) = sink else {
        return false;
    };
    tracing::debug!("demo: another launch hands over through the probe");
    sink(handoff);
    true
}

impl HandoffPort for DemoHandoff {
    fn on_handoff(&self, sink: HandoffSink) {
        if let Some(socket) = &self.socket {
            socket.on_handoff(Arc::clone(&sink));
        }
        if let Ok(mut slot) = self.sink.lock() {
            *slot = Some(sink);
        }
    }
}
