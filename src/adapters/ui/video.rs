use std::cell::Cell;
#[cfg(feature = "video")]
use std::cell::RefCell;
use std::path::Path;
#[cfg(feature = "video")]
use std::path::PathBuf;
use std::time::Duration;

use slint::ComponentHandle;
#[cfg(feature = "video")]
use slint::{Rgb8Pixel, SharedPixelBuffer};

use super::props::{BoolProp, IntProp, UiProps};
use crate::commands::messages::UserMessageKind;

thread_local! {
    #[cfg(feature = "video")]
    static PLAYBACK: RefCell<Option<Session>> = const { RefCell::new(None) };
    #[cfg(feature = "video")]
    static LAST_GENERATION: Cell<u64> = const { Cell::new(0) };
    static PLAYING: Cell<bool> = const { Cell::new(false) };
    static MUTED: Cell<bool> = const { Cell::new(false) };
}

#[cfg(feature = "video")]
use crate::adapters::video::playback::Playback;

#[cfg(feature = "video")]
struct Session {
    generation: u64,
    path: PathBuf,
    playback: Playback,
}

#[cfg(feature = "video")]
pub enum Update {
    Ready(Option<Duration>),
    Frame {
        buffer: SharedPixelBuffer<Rgb8Pixel>,
        position: Duration,
    },
    Ended,
    Failed,
}

pub fn open<W>(window: &W, weak: &slint::Weak<W>, path: &Path)
where
    W: ComponentHandle + UiProps + 'static,
{
    if is_open(path) {
        return;
    }
    stop();
    window.set_int(IntProp::VideoPositionMs, 0);
    window.set_int(IntProp::VideoDurationMs, 0);
    window.clear_video_frame();
    MUTED.set(false);
    window.set_bool(BoolProp::VideoMuted, false);
    start(window, weak, path);
}

pub fn close<W: UiProps>(window: &W) {
    stop();
    PLAYING.set(false);
    window.set_bool(BoolProp::VideoPlaying, false);
    window.set_int(IntProp::VideoPositionMs, 0);
    window.clear_video_frame();
}

#[cfg(feature = "video")]
fn is_open(path: &Path) -> bool {
    PLAYBACK.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|session| session.path == path)
    })
}

#[cfg(not(feature = "video"))]
fn is_open(_path: &Path) -> bool {
    false
}

#[cfg(feature = "video")]
fn is_current(generation: u64) -> bool {
    PLAYBACK.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|session| session.generation == generation)
    })
}

#[cfg(feature = "video")]
fn next_generation() -> u64 {
    let generation = LAST_GENERATION.get().wrapping_add(1);
    LAST_GENERATION.set(generation);
    generation
}

#[cfg(feature = "video")]
pub fn stop() {
    PLAYBACK.with(|cell| cell.borrow_mut().take());
}

#[cfg(not(feature = "video"))]
pub fn stop() {}

pub fn set_playing<W: UiProps>(window: &W, playing: bool) {
    #[cfg(feature = "video")]
    PLAYBACK.with(|cell| {
        if let Some(session) = cell.borrow().as_ref() {
            session.set_playing(playing);
        }
    });
    PLAYING.set(playing);
    window.set_bool(BoolProp::VideoPlaying, playing);
}

pub fn toggle<W: UiProps>(window: &W) {
    set_playing(window, !PLAYING.get());
}

pub fn toggle_muted<W: UiProps>(window: &W) {
    let muted = !MUTED.get();
    MUTED.set(muted);
    #[cfg(feature = "video")]
    PLAYBACK.with(|cell| {
        if let Some(session) = cell.borrow().as_ref() {
            session.playback.set_muted(muted);
        }
    });
    window.set_bool(BoolProp::VideoMuted, muted);
}

pub fn seek<W: UiProps>(window: &W, position: Duration) {
    #[cfg(feature = "video")]
    PLAYBACK.with(|cell| {
        if let Some(session) = cell.borrow().as_ref() {
            session.seek(position);
        }
    });
    window.set_int(IntProp::VideoPositionMs, millis(position));
}

#[cfg(feature = "video")]
pub fn apply<W: UiProps>(window: &W, generation: u64, update: Update) {
    if !is_current(generation) {
        return;
    }
    match update {
        Update::Ready(duration) => {
            window.set_int(IntProp::VideoDurationMs, duration.map_or(0, millis));
            PLAYING.set(true);
            window.set_bool(BoolProp::VideoPlaying, true);
        }
        Update::Frame { buffer, position } => {
            window.apply_video_frame(buffer);
            window.set_int(IntProp::VideoPositionMs, millis(position));
        }
        Update::Ended => {
            PLAYING.set(false);
            window.set_bool(BoolProp::VideoPlaying, false);
        }
        Update::Failed => fail(window),
    }
}

fn fail<W: UiProps>(window: &W) {
    PLAYING.set(false);
    window.set_bool(BoolProp::VideoPlaying, false);
    window.set_video_error(UserMessageKind::VideoPlaybackFailed);
}

pub fn millis_to_duration(millis: Option<usize>) -> Duration {
    Duration::from_millis(millis.unwrap_or(0) as u64)
}

fn millis(duration: Duration) -> i32 {
    i32::try_from(duration.as_millis()).unwrap_or(i32::MAX)
}

#[cfg(feature = "video")]
impl Session {
    fn set_playing(&self, playing: bool) {
        if playing {
            self.playback.play();
        } else {
            self.playback.pause();
        }
    }

    fn seek(&self, position: Duration) {
        self.playback.seek(position);
    }
}

#[cfg(not(feature = "video"))]
fn start<W>(window: &W, _weak: &slint::Weak<W>, _path: &Path)
where
    W: ComponentHandle + UiProps + 'static,
{
    fail(window);
}

#[cfg(feature = "video")]
fn start<W>(window: &W, weak: &slint::Weak<W>, path: &Path)
where
    W: ComponentHandle + UiProps + 'static,
{
    use crate::adapters::video::player::{self, PlayerEvent};

    let generation = next_generation();
    let weak = weak.clone();
    let sink = Box::new(move |event: PlayerEvent<'_>| {
        let update = match event {
            PlayerEvent::Ready { duration } => Update::Ready(duration),
            PlayerEvent::Ended => Update::Ended,
            PlayerEvent::Failed => Update::Failed,
            PlayerEvent::Frame {
                rgb,
                width,
                height,
                position,
            } => Update::Frame {
                buffer: SharedPixelBuffer::clone_from_slice(rgb, width, height),
                position,
            },
        };
        if weak
            .upgrade_in_event_loop(move |window| apply(&window, generation, update))
            .is_err()
        {
            tracing::debug!("the event loop is gone, dropping a video update");
        }
    });

    let Some(playback) = player::start(path, sink) else {
        fail(window);
        return;
    };
    playback.play();
    PLAYBACK.with(|cell| {
        *cell.borrow_mut() = Some(Session {
            generation,
            path: path.to_path_buf(),
            playback,
        });
    });
}
