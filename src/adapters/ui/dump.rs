use std::sync::OnceLock;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::oneshot;

#[derive(Serialize)]
pub struct ReactionRowDump {
    pub key: String,
    pub label: String,
    pub count: i32,
    pub mine: bool,
    pub send: &'static str,
    pub overflow: bool,
    pub hidden_reactors: i32,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct TimelineRowDump {
    pub row: usize,
    pub unique_id: String,
    pub local_id: String,
    pub event_id: String,
    pub sender: String,
    pub sender_id: String,
    pub body: String,
    pub timestamp: String,
    pub message_type: &'static str,
    pub preview_kind: &'static str,
    pub service_kind: &'static str,
    pub service_target: String,
    pub media_state: &'static str,
    pub media_failure: &'static str,
    pub send_state: &'static str,
    pub send_progress: f32,
    pub is_own: bool,
    pub edited: bool,
    pub first_unread: bool,
    pub needs_media: bool,
    pub has_avatar: bool,
    pub image_width: i32,
    pub image_height: i32,
    pub duration: String,
    pub filename: String,
    pub size: String,
    pub audio_kind: &'static str,
    pub waveform: Vec<f32>,
    pub has_reply: bool,
    pub reply_event_id: String,
    pub reply_sender: String,
    pub reply_body: String,
    pub reactions: Vec<ReactionRowDump>,
}

#[derive(Serialize)]
pub struct AudioDump {
    pub state: &'static str,
    pub output: &'static str,
    pub event_id: String,
    pub room_id: String,
    pub kind: &'static str,
    pub sender: String,
    pub title: String,
    pub position_ms: i32,
    pub duration_ms: i32,
}

#[derive(Serialize)]
pub struct TimelineDump {
    pub selected_room_id: String,
    pub selected_room_name: String,
    pub generation: i32,
    pub timeline_token: i32,
    pub prepend_token: i32,
    pub anchor_index: i32,
    pub focus_event_id: String,
    pub rows: Vec<TimelineRowDump>,
    pub audio: AudioDump,
}

type Requester = Box<dyn Fn(oneshot::Sender<TimelineDump>) + Send + Sync>;

static REQUESTER: OnceLock<Requester> = OnceLock::new();

#[cfg(not(feature = "interpreted"))]
pub fn install(requester: Requester) {
    if REQUESTER.set(requester).is_err() {
        tracing::debug!("the timeline dump was already installed");
    }
}

#[derive(Clone, Copy)]
pub enum Poke {
    ToggleAudio,
    SeekAudio(Duration),
}

type Poker = Box<dyn Fn(Poke) + Send + Sync>;

static POKER: OnceLock<Poker> = OnceLock::new();

#[cfg(not(feature = "interpreted"))]
pub fn install_pokes(poker: Poker) {
    if POKER.set(poker).is_err() {
        tracing::debug!("the probe pokes were already installed");
    }
}

pub fn poke(poke: Poke) -> bool {
    POKER.get().is_some_and(|poker| {
        poker(poke);
        true
    })
}

pub fn request() -> Option<oneshot::Receiver<TimelineDump>> {
    let requester = REQUESTER.get()?;
    let (tx, rx) = oneshot::channel();
    requester(tx);
    Some(rx)
}
