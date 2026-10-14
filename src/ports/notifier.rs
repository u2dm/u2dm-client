use std::sync::Arc;

use crate::domain::room::RoomId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopNotice {
    pub key: RoomId,
    pub summary: String,
    pub body: String,
    pub open_label: String,
    pub sound: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeActivation {
    pub key: RoomId,
    pub token: Option<String>,
}

pub type ActivationSink = Arc<dyn Fn(NoticeActivation) + Send + Sync>;

pub trait NotifierPort: Send + Sync {
    fn show(&self, notice: DesktopNotice);
    fn withdraw(&self, key: &RoomId);
    fn withdraw_all(&self);
    fn on_activation(&self, sink: ActivationSink);
}
