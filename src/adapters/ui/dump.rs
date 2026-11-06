mod call;
mod timeline;

use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Duration;

use serde::Serialize;
use slint::{ComponentHandle, LogicalSize, Model};
use tokio::sync::oneshot;

use self::call::{CallDump, call_view};
use self::timeline::{TimelineRowDump, timeline_row};
use super::backend::{UiBackend, in_event_loop};
use super::fields::LogLineFields;
use super::props::{BoolProp, EnumProp, IntProp, StringProp, UiProps};
use super::{activity, audio, call_window, context_press, touchpad_pinch};

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
pub struct PollPermissionsDump {
    pub vote: bool,
    pub end: bool,
    pub start: bool,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct MessagePermissionsDump {
    pub send_messages: bool,
    pub react: bool,
    pub send_stickers: bool,
    pub delete_own: bool,
    pub delete_others: bool,
    pub pin: bool,
}

#[derive(Serialize)]
pub struct RoomMenuDump {
    pub key: String,
    pub room_id: String,
    pub shown: bool,
}

#[derive(Serialize)]
pub struct RoomLogDump {
    pub visible: bool,
    pub room_id: String,
    pub name: String,
    pub rows: usize,
    pub dropped: i32,
    pub lines_landed: i32,
    pub newest: Vec<String>,
}

#[derive(Serialize)]
pub struct TimelineDump {
    pub selected_room_id: String,
    pub selected_room_name: String,
    pub selected_room_encrypted: bool,
    pub selected_room_readable_by_anyone: bool,
    pub poll_permissions: PollPermissionsDump,
    pub message_permissions: MessagePermissionsDump,
    pub generation: i32,
    pub timeline_token: i32,
    pub prepend_token: i32,
    pub anchor_index: i32,
    pub focus_event_id: String,
    pub pinned: PinnedDump,
    pub rows: Vec<TimelineRowDump>,
    pub audio: AudioDump,
    pub room_menu: RoomMenuDump,
    pub room_log: RoomLogDump,
    pub call: CallDump,
    pub title: String,
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
    WindowSize(f32, f32),
    UserActivity(bool),
    SwipeTravel(i32),
    TouchpadPinch(i32),
    ContextPress,
    CloseCallWindow,
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
    install(Box::new(in_event_loop::<B, oneshot::Sender<TimelineDump>>(
        window,
        "the timeline dump",
        |window, reply| drop(reply.send(collect::<B>(window))),
    )));
    install_pokes(Box::new(in_event_loop::<B, Poke>(
        window,
        "a probe poke",
        apply_poke::<B>,
    )));
}

fn apply_poke<B: UiBackend>(window: &B::Window, poke: Poke) {
    match poke {
        Poke::ToggleAudio => audio::toggle(window),
        Poke::SeekAudio(position) => audio::seek(window, position),
        Poke::WindowFocus(focused) => {
            window.set_bool(BoolProp::WindowFocused, focused);
            window.window().request_redraw();
        }
        Poke::WindowSize(width, height) => {
            window.window().set_size(LogicalSize::new(width, height));
        }
        Poke::UserActivity(active) => activity::report(window, active),
        Poke::SwipeTravel(px) => window.set_int(IntProp::SwipeTravel, px),
        Poke::TouchpadPinch(permille) => touchpad_pinch::publish::<B>(window, permille),
        Poke::ContextPress => context_press::announce(window),
        Poke::CloseCallWindow => call_window::request_close::<B>(),
    }
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
        selected_room_readable_by_anyone: window.get_bool(BoolProp::SelectedRoomReadableByAnyone),
        poll_permissions: PollPermissionsDump {
            vote: window.get_bool(BoolProp::MayVote),
            end: window.get_bool(BoolProp::MayEndPolls),
            start: window.get_bool(BoolProp::MayStartPolls),
        },
        message_permissions: MessagePermissionsDump {
            send_messages: window.get_bool(BoolProp::MaySendMessages),
            react: window.get_bool(BoolProp::MayReact),
            send_stickers: window.get_bool(BoolProp::MaySendStickers),
            delete_own: window.get_bool(BoolProp::MayDeleteOwn),
            delete_others: window.get_bool(BoolProp::MayDeleteOthers),
            pin: window.get_bool(BoolProp::MayPin),
        },
        generation: window.get_int(IntProp::SelectedGeneration),
        timeline_token: window.get_int(IntProp::TimelineToken),
        prepend_token: window.get_int(IntProp::PrependToken),
        anchor_index: window.get_int(IntProp::AnchorIndex),
        focus_event_id: window.get_string(StringProp::FocusEventId).to_string(),
        pinned: pinned_view(window),
        rows,
        audio: audio_view(window),
        room_menu: room_menu_view(window),
        room_log: room_log_view::<B>(window),
        call: call_view::<B>(window),
        title: window.get_string(StringProp::WindowTitleText).to_string(),
    }
}

const NEWEST_LOG_ROWS: usize = 12;

fn room_log_view<B: UiBackend>(window: &B::Window) -> RoomLogDump {
    let lines = Rc::clone(&B::models().room_log);
    let rows = lines.row_count();
    let newest = lines
        .iter()
        .skip(rows.saturating_sub(NEWEST_LOG_ROWS))
        .map(|line| {
            format!(
                "{} {} {}: {}",
                line.time(),
                line.level_name(),
                line.target(),
                line.text()
            )
        })
        .collect();
    RoomLogDump {
        visible: window.get_bool(BoolProp::RoomLogVisible),
        room_id: window.get_string(StringProp::RoomLogRoomId).to_string(),
        name: window.get_string(StringProp::RoomLogName).to_string(),
        rows,
        dropped: window.get_int(IntProp::RoomLogDropped),
        lines_landed: window.get_int(IntProp::RoomLogLinesLanded),
        newest,
    }
}

fn room_menu_view(window: &impl UiProps) -> RoomMenuDump {
    let key = window.get_string(StringProp::RoomMenuKey).to_string();
    let room_id = window.get_string(StringProp::RoomMenuRoomId).to_string();
    RoomMenuDump {
        shown: !key.is_empty() && key == room_id,
        key,
        room_id,
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
