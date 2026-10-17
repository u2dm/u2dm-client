use std::cell::Cell;
#[cfg(feature = "video")]
use std::cell::RefCell;
#[cfg(feature = "video")]
use std::path::PathBuf;
#[cfg(feature = "video")]
use std::time::Duration;

#[cfg(feature = "video")]
use slint::ComponentHandle;

#[cfg(feature = "video")]
use super::audio::preferred_sound;
#[cfg(feature = "video")]
use crate::adapters::video::audio_player::{self, AudioEvent};
#[cfg(feature = "video")]
use crate::adapters::video::playback::Playback;

#[cfg(feature = "video")]
const SOUND_THEME_DIR: &str = "sounds/freedesktop/stereo";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Incoming,
    Ringback,
}

impl Tone {
    #[cfg(feature = "video")]
    const fn file(self) -> &'static str {
        match self {
            Self::Incoming => "phone-incoming-call.oga",
            Self::Ringback => "phone-outgoing-calling.oga",
        }
    }
}

thread_local! {
    #[cfg(feature = "video")]
    static RINGING: RefCell<Option<Ringing>> = const { RefCell::new(None) };
    static SOUNDING: Cell<Option<Tone>> = const { Cell::new(None) };
    static GENERATION: Cell<u64> = const { Cell::new(0) };
}

#[cfg(feature = "video")]
struct Ringing {
    generation: u64,
    playback: Playback,
}

pub fn sounding() -> Option<Tone> {
    SOUNDING.get()
}

pub fn stop_if(tone: Tone) {
    if SOUNDING.get() == Some(tone) {
        stop();
    }
}

pub fn stop() {
    if SOUNDING.take().is_none() {
        return;
    }
    GENERATION.set(GENERATION.get().wrapping_add(1));
    #[cfg(feature = "video")]
    RINGING.with(|cell| drop(cell.borrow_mut().take()));
    tracing::debug!("the ringtone stopped");
}

#[cfg(not(feature = "video"))]
pub fn play<W>(_weak: &slint::Weak<W>, tone: Tone) -> bool
where
    W: slint::ComponentHandle + 'static,
{
    tracing::debug!(?tone, "this build plays no ringtone");
    false
}

#[cfg(feature = "video")]
pub fn play<W>(weak: &slint::Weak<W>, tone: Tone) -> bool
where
    W: ComponentHandle + 'static,
{
    if SOUNDING.get() == Some(tone) {
        return true;
    }
    stop();
    let Some(path) = sound_file(tone) else {
        tracing::debug!(?tone, "the sound theme has no ringtone");
        return false;
    };
    let generation = GENERATION.get();
    let weak = weak.clone();
    let sink = Box::new(move |event: AudioEvent| {
        if !matches!(event, AudioEvent::Ended) {
            return;
        }
        if weak
            .upgrade_in_event_loop(move |_| rewind(generation))
            .is_err()
        {
            tracing::debug!("the event loop is gone, the ringtone stops looping");
        }
    });
    let Some(playback) = audio_player::start(&path, preferred_sound(), sink) else {
        return false;
    };
    playback.play();
    RINGING.with(|cell| {
        *cell.borrow_mut() = Some(Ringing {
            generation,
            playback,
        });
    });
    SOUNDING.set(Some(tone));
    tracing::debug!(?tone, path = %path.display(), "the ringtone plays");
    true
}

#[cfg(feature = "video")]
fn rewind(generation: u64) {
    RINGING.with(|cell| {
        if let Some(ringing) = cell.borrow().as_ref()
            && ringing.generation == generation
        {
            ringing.playback.seek(Duration::ZERO);
            ringing.playback.play();
        }
    });
}

#[cfg(all(feature = "video", unix))]
fn sound_file(tone: Tone) -> Option<PathBuf> {
    xdg::BaseDirectories::new().find_data_file(PathBuf::from(SOUND_THEME_DIR).join(tone.file()))
}

#[cfg(all(feature = "video", not(unix)))]
fn sound_file(_tone: Tone) -> Option<PathBuf> {
    None
}
