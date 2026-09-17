use std::cell::{Cell, RefCell};
use std::path::Path;
use std::time::Duration;

use slint::ComponentHandle;

use super::props::{BoolProp, IntProp, UiProps, send_command};
#[cfg(feature = "video")]
use crate::adapters::video::audio_player::{self, AudioEvent, Sound};
#[cfg(feature = "video")]
use crate::adapters::video::playback::Playback;
use crate::app::input::CommandSender;
use crate::commands::ui::{AudioEnd, UiCommand};

thread_local! {
    #[cfg(feature = "video")]
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
    static REQUEST: Cell<u64> = const { Cell::new(0) };
    static PLAYING: Cell<bool> = const { Cell::new(false) };
    #[cfg(feature = "video")]
    static SILENT_ONLY: Cell<bool> = const { Cell::new(false) };
    static COMMANDS: RefCell<Option<CommandSender>> = const { RefCell::new(None) };
}

#[cfg(feature = "video")]
struct Session {
    request: u64,
    playback: Playback,
}

#[cfg(feature = "video")]
#[derive(Clone, Copy)]
pub enum Update {
    Ready {
        duration: Option<Duration>,
        silent: bool,
    },
    Position(Duration),
    Ended,
    Failed,
}

pub fn install_commands(commands: &CommandSender) {
    COMMANDS.with(|cell| *cell.borrow_mut() = Some(commands.clone()));
}

#[cfg(feature = "demo")]
pub fn prefer_silent() {
    #[cfg(feature = "video")]
    SILENT_ONLY.set(true);
}

pub fn prepare<W: UiProps>(window: &W, request: u64, duration: Option<Duration>) {
    if REQUEST.get() == request {
        return;
    }
    stop();
    REQUEST.set(request);
    PLAYING.set(false);
    window.set_bool(BoolProp::AudioPlaying, false);
    window.set_bool(BoolProp::AudioSilent, false);
    window.set_int(IntProp::AudioPositionMs, 0);
    window.set_int(IntProp::AudioDurationMs, duration.map_or(0, millis));
}

pub fn open<W>(window: &W, weak: &slint::Weak<W>, request: u64, path: &Path)
where
    W: ComponentHandle + UiProps + 'static,
{
    if REQUEST.get() != request || is_open(request) {
        return;
    }
    start(window, weak, request, path);
}

pub fn close<W: UiProps>(window: &W) {
    stop();
    REQUEST.set(0);
    PLAYING.set(false);
    window.set_bool(BoolProp::AudioPlaying, false);
    window.set_bool(BoolProp::AudioSilent, false);
    window.set_int(IntProp::AudioPositionMs, 0);
    window.set_int(IntProp::AudioDurationMs, 0);
}

pub fn pause<W: UiProps>(window: &W) {
    if PLAYING.get() {
        set_playing(window, false);
    }
}

pub fn toggle<W: UiProps>(window: &W) {
    set_playing(window, !PLAYING.get());
}

fn set_playing<W: UiProps>(window: &W, playing: bool) {
    #[cfg(feature = "video")]
    SESSION.with(|cell| {
        if let Some(session) = cell.borrow().as_ref() {
            if playing {
                session.playback.play();
            } else {
                session.playback.pause();
            }
        }
    });
    PLAYING.set(playing);
    window.set_bool(BoolProp::AudioPlaying, playing);
}

pub fn seek<W: UiProps>(window: &W, position: Duration) {
    #[cfg(feature = "video")]
    SESSION.with(|cell| {
        if let Some(session) = cell.borrow().as_ref() {
            session.playback.seek(position);
        }
    });
    window.set_int(IntProp::AudioPositionMs, millis(position));
}

#[cfg(feature = "video")]
pub fn apply<W: UiProps>(window: &W, request: u64, update: Update) {
    if REQUEST.get() != request {
        return;
    }
    match update {
        Update::Ready { duration, silent } => {
            if let Some(duration) = duration {
                window.set_int(IntProp::AudioDurationMs, millis(duration));
            }
            window.set_bool(BoolProp::AudioSilent, silent);
        }
        Update::Position(position) => {
            window.set_int(IntProp::AudioPositionMs, millis(position));
        }
        Update::Ended => finish(window, request, AudioEnd::Finished),
        Update::Failed => finish(window, request, AudioEnd::Failed),
    }
}

fn finish<W: UiProps>(window: &W, request: u64, end: AudioEnd) {
    PLAYING.set(false);
    window.set_bool(BoolProp::AudioPlaying, false);
    COMMANDS.with(|cell| {
        if let Some(commands) = cell.borrow().as_ref() {
            send_command(commands, UiCommand::AudioEnded { request, end });
        }
    });
}

fn millis(duration: Duration) -> i32 {
    i32::try_from(duration.as_millis()).unwrap_or(i32::MAX)
}

#[cfg(feature = "video")]
fn is_open(request: u64) -> bool {
    SESSION.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|session| session.request == request)
    })
}

#[cfg(not(feature = "video"))]
fn is_open(_request: u64) -> bool {
    false
}

#[cfg(feature = "video")]
fn stop() {
    SESSION.with(|cell| cell.borrow_mut().take());
}

#[cfg(not(feature = "video"))]
fn stop() {}

#[cfg(not(feature = "video"))]
fn start<W>(window: &W, _weak: &slint::Weak<W>, request: u64, _path: &Path)
where
    W: ComponentHandle + UiProps + 'static,
{
    finish(window, request, AudioEnd::Failed);
}

#[cfg(feature = "video")]
fn start<W>(window: &W, weak: &slint::Weak<W>, request: u64, path: &Path)
where
    W: ComponentHandle + UiProps + 'static,
{
    let weak = weak.clone();
    let sink = Box::new(move |event: AudioEvent| {
        let update = match event {
            AudioEvent::Ready { duration, sound } => Update::Ready {
                duration,
                silent: sound == Sound::Silent,
            },
            AudioEvent::Position(position) => Update::Position(position),
            AudioEvent::Ended => Update::Ended,
            AudioEvent::Failed => Update::Failed,
        };
        if weak
            .upgrade_in_event_loop(move |window| apply(&window, request, update))
            .is_err()
        {
            tracing::debug!("the event loop is gone, dropping an audio update");
        }
    });
    let preferred = if SILENT_ONLY.get() {
        Sound::Silent
    } else {
        Sound::Device
    };
    let playback = audio_player::start(path, preferred, sink);
    playback.play();
    PLAYING.set(true);
    window.set_bool(BoolProp::AudioPlaying, true);
    SESSION.with(|cell| {
        *cell.borrow_mut() = Some(Session { request, playback });
    });
}
