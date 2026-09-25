use std::sync::OnceLock;
use std::time::Duration;

use serde::Serialize;
use slint::{ComponentHandle, Model};
use tokio::sync::oneshot;

use super::audio;
use super::backend::UiBackend;
use super::fields::{MessageFields, PollAnswerFields, ReactionFields};
use super::props::{BoolProp, EnumProp, IntProp, StringProp, UiProps};

#[derive(Serialize)]
pub struct ReactionRowDump {
    pub key: String,
    pub label: String,
    pub count: i32,
    pub mine: bool,
    pub send: String,
    pub overflow: bool,
    pub hidden_reactors: i32,
}

#[derive(Serialize)]
pub struct PollAnswerRowDump {
    pub id: String,
    pub label: String,
    pub count: i32,
    pub share: f32,
    pub mine: bool,
    pub leading: bool,
    pub votable: bool,
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
    pub sent_at: String,
    pub message_type: String,
    pub preview_kind: String,
    pub service_kind: String,
    pub service_target: String,
    pub media_state: String,
    pub media_failure: String,
    pub send_state: String,
    pub send_progress: f32,
    pub delivery: String,
    pub readers: String,
    pub hidden_readers: i32,
    pub reader_count: i32,
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
    pub audio_kind: String,
    pub waveform: Vec<f32>,
    pub has_reply: bool,
    pub reply_event_id: String,
    pub reply_sender: String,
    pub reply_body: String,
    pub reactions: Vec<ReactionRowDump>,
    pub poll_phase: String,
    pub poll_choices: i32,
    pub poll_voters: i32,
    pub poll_answers: Vec<PollAnswerRowDump>,
}

#[derive(Serialize)]
pub struct AudioDump {
    pub state: &'static str,
    pub output: &'static str,
    pub event_id: String,
    pub room_id: String,
    pub kind: String,
    pub sender: String,
    pub title: String,
    pub position_ms: i32,
    pub duration_ms: i32,
}

#[derive(Serialize)]
pub struct PinnedDump {
    pub count: i32,
    pub index: i32,
    pub event_id: String,
    pub kind: String,
    pub body: String,
}

#[derive(Serialize)]
pub struct TimelineDump {
    pub selected_room_id: String,
    pub selected_room_name: String,
    pub selected_room_encrypted: bool,
    pub generation: i32,
    pub timeline_token: i32,
    pub prepend_token: i32,
    pub anchor_index: i32,
    pub focus_event_id: String,
    pub pinned: PinnedDump,
    pub rows: Vec<TimelineRowDump>,
    pub audio: AudioDump,
}

type Requester = Box<dyn Fn(oneshot::Sender<TimelineDump>) + Send + Sync>;

static REQUESTER: OnceLock<Requester> = OnceLock::new();

fn install(requester: Requester) {
    if REQUESTER.set(requester).is_err() {
        tracing::debug!("the timeline dump was already installed");
    }
}

#[derive(Clone, Copy)]
pub enum Poke {
    ToggleAudio,
    SeekAudio(Duration),
    WindowFocus(bool),
    SwipeTravel(i32),
}

type Poker = Box<dyn Fn(Poke) + Send + Sync>;

static POKER: OnceLock<Poker> = OnceLock::new();

fn install_pokes(poker: Poker) {
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

pub fn install_probe<B: UiBackend>(window: &B::Window) {
    let weak = window.as_weak();
    let dump_weak = weak.clone();
    install(Box::new(move |reply| {
        let queued = dump_weak.upgrade_in_event_loop(move |window| {
            drop(reply.send(collect::<B>(&window)));
        });
        if let Err(e) = queued {
            tracing::debug!("the timeline dump could not reach the event loop: {e}");
        }
    }));
    install_pokes(Box::new(move |poke| {
        let queued = weak.upgrade_in_event_loop(move |window| match poke {
            Poke::ToggleAudio => audio::toggle(&window),
            Poke::SeekAudio(position) => audio::seek(&window, position),
            Poke::WindowFocus(focused) => window.set_bool(BoolProp::WindowFocused, focused),
            Poke::SwipeTravel(px) => window.set_int(IntProp::SwipeTravel, px),
        });
        if let Err(e) = queued {
            tracing::debug!("a probe poke could not reach the event loop: {e}");
        }
    }));
}

fn collect<B: UiBackend>(window: &B::Window) -> TimelineDump {
    let rows = B::with_timeline(|model| {
        model
            .iter()
            .enumerate()
            .map(|(index, entry)| timeline_row::<B>(index, &entry))
            .collect()
    });
    TimelineDump {
        selected_room_id: window.get_string(StringProp::SelectedRoomId).to_string(),
        selected_room_name: window.get_string(StringProp::SelectedRoomName).to_string(),
        selected_room_encrypted: window.get_bool(BoolProp::SelectedRoomEncrypted),
        generation: window.get_int(IntProp::SelectedGeneration),
        timeline_token: window.get_int(IntProp::TimelineToken),
        prepend_token: window.get_int(IntProp::PrependToken),
        anchor_index: window.get_int(IntProp::AnchorIndex),
        focus_event_id: window.get_string(StringProp::FocusEventId).to_string(),
        pinned: pinned_view(window),
        rows,
        audio: audio_view(window),
    }
}

fn pinned_view(window: &impl UiProps) -> PinnedDump {
    PinnedDump {
        count: window.get_int(IntProp::PinnedCount),
        index: window.get_int(IntProp::PinnedIndex),
        event_id: window.get_string(StringProp::PinnedEventId).to_string(),
        kind: window.get_enum(EnumProp::PinnedKind).to_string(),
        body: window.get_string(StringProp::PinnedBody).to_string(),
    }
}

fn timeline_row<B: UiBackend>(row: usize, entry: &B::Message) -> TimelineRowDump {
    TimelineRowDump {
        row,
        unique_id: entry.unique_id().to_owned(),
        local_id: entry.local_id().to_owned(),
        event_id: entry.event_id().to_owned(),
        sender: entry.sender().to_owned(),
        sender_id: entry.sender_id().to_owned(),
        body: entry.body().to_owned(),
        timestamp: entry.timestamp().to_owned(),
        sent_at: entry.sent_at().to_owned(),
        message_type: entry.message_type().to_owned(),
        preview_kind: entry.preview_kind().to_owned(),
        service_kind: entry.service_kind().to_owned(),
        service_target: entry.service_target().to_owned(),
        media_state: entry.media_state().to_owned(),
        media_failure: entry.media_failure().to_owned(),
        send_state: entry.send_state().to_owned(),
        send_progress: entry.send_progress(),
        delivery: entry.delivery().to_owned(),
        readers: entry.readers().to_owned(),
        hidden_readers: entry.hidden_readers(),
        reader_count: entry.reader_count(),
        is_own: entry.is_own(),
        edited: entry.edited(),
        first_unread: entry.first_unread(),
        needs_media: entry.needs_media(),
        has_avatar: entry.has_avatar(),
        image_width: entry.image_width(),
        image_height: entry.image_height(),
        duration: entry.duration().to_owned(),
        filename: entry.filename().to_owned(),
        size: entry.size().to_owned(),
        audio_kind: entry.audio_kind().to_owned(),
        waveform: entry.waveform(),
        has_reply: entry.has_reply(),
        reply_event_id: entry.reply_event_id().to_owned(),
        reply_sender: entry.reply_sender().to_owned(),
        reply_body: entry.reply_body().to_owned(),
        reactions: entry
            .reactions()
            .iter()
            .map(|reaction| reaction_row::<B>(&reaction))
            .collect(),
        poll_phase: entry.poll_phase().to_owned(),
        poll_choices: entry.poll_choices(),
        poll_voters: entry.poll_voters(),
        poll_answers: entry
            .poll_answers()
            .iter()
            .map(|answer| poll_answer_row::<B>(&answer))
            .collect(),
    }
}

fn poll_answer_row<B: UiBackend>(entry: &B::PollAnswer) -> PollAnswerRowDump {
    PollAnswerRowDump {
        id: entry.id().to_owned(),
        label: entry.label().to_owned(),
        count: entry.count(),
        share: entry.share(),
        mine: entry.mine(),
        leading: entry.leading(),
        votable: entry.votable(),
    }
}

fn reaction_row<B: UiBackend>(entry: &B::Reaction) -> ReactionRowDump {
    ReactionRowDump {
        key: entry.key().to_owned(),
        label: entry.label().to_owned(),
        count: entry.count(),
        mine: entry.mine(),
        send: entry.send().to_owned(),
        overflow: entry.overflow(),
        hidden_reactors: entry.hidden_reactors(),
    }
}

fn audio_view(window: &impl UiProps) -> AudioDump {
    let state = match (
        window.get_bool(BoolProp::AudioVisible),
        window.get_bool(BoolProp::AudioLoading),
        window.get_bool(BoolProp::AudioPlaying),
    ) {
        (false, _, _) => "hidden",
        (true, true, _) => "loading",
        (true, false, true) => "playing",
        (true, false, false) => "paused",
    };
    AudioDump {
        state,
        output: if window.get_bool(BoolProp::AudioSilent) {
            "silent"
        } else {
            "device"
        },
        event_id: window.get_string(StringProp::AudioEventId).to_string(),
        room_id: window.get_string(StringProp::AudioRoomId).to_string(),
        kind: window.get_enum(EnumProp::AudioKind).to_string(),
        sender: window.get_string(StringProp::AudioSender).to_string(),
        title: window.get_string(StringProp::AudioTitle).to_string(),
        position_ms: window.get_int(IntProp::AudioPositionMs),
        duration_ms: window.get_int(IntProp::AudioDurationMs),
    }
}
