use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    pub link: Option<String>,
    pub token: Option<String>,
}

pub type HandoffSink = Arc<dyn Fn(Handoff) + Send + Sync>;

pub trait HandoffPort: Send + Sync {
    fn on_handoff(&self, sink: HandoffSink);
}
