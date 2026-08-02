use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, DynamicImage, Frames, ImageDecoder, RgbaImage};
use slint::{Image, Timer, TimerMode};

use super::waiters::DecodeOutcome;
use super::workers::Lane;
use super::{DISPLAY_MAX_DIMENSION, cache, image_from_rgba, waiters};

const ANIMATION_MEMORY_BUDGET: usize = 128 * 1024 * 1024;
const ANIM_PER_ITEM_BUDGET: usize = 32 * 1024 * 1024;
const ANIM_MAX_DIMENSION: u32 = 2048;
const ANIM_MAX_FRAMES: usize = 600;
const ANIM_MAX_SOURCE_PIXELS: u64 = 128 * 1024 * 1024;
const ANIM_CANVAS_BYTES: u64 = 4 * ANIM_MAX_DIMENSION as u64 * ANIM_MAX_DIMENSION as u64;
const ANIM_CONCURRENT_CANVASES: u64 = 4;
const ANIM_MAX_ALLOC: u64 = ANIM_CONCURRENT_CANVASES * ANIM_CANVAS_BYTES;
const MAX_ACTIVE_ANIMATIONS: usize = 16;

const GIF_INSTANT_DELAY: Duration = Duration::from_millis(10);
const GIF_DEFAULT_DELAY: Duration = Duration::from_millis(100);

thread_local! {
    static ANIMATIONS: RefCell<AnimationState> = RefCell::new(AnimationState::default());
    static ANIMATION_TIMER: Timer = Timer::default();
    static ANIMATION_TICK_FN: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct AnimationState {
    clips: HashMap<PathBuf, Option<Rc<Animation>>>,
    playbacks: HashMap<String, Playback>,
}

impl AnimationState {
    fn retained_bytes(&self) -> usize {
        self.clips
            .values()
            .filter_map(Option::as_ref)
            .map(|animation| animation.bytes)
            .sum()
    }

    fn start_playback(
        &mut self,
        unique_id: &str,
        path: &Path,
        animation: &Animation,
    ) -> PlaybackStart {
        if self.playbacks.contains_key(unique_id) {
            return PlaybackStart::AlreadyRunning;
        }
        if self.playbacks.len() >= MAX_ACTIVE_ANIMATIONS {
            return PlaybackStart::AtCapacity;
        }
        self.playbacks.insert(
            unique_id.to_owned(),
            Playback {
                path: path.to_path_buf(),
                frame: 0,
                next_at: Instant::now() + animation.delay(0),
                row_hint: 0,
            },
        );
        PlaybackStart::Started
    }
}

enum PlaybackStart {
    Started,
    AlreadyRunning,
    AtCapacity,
}

struct Animation {
    frames: Vec<Image>,
    delays: Vec<Duration>,
    bytes: usize,
}

impl Animation {
    fn frame(&self, index: usize) -> Option<&Image> {
        self.frames.get(index)
    }

    fn delay(&self, index: usize) -> Duration {
        self.delays.get(index).copied().unwrap_or(GIF_DEFAULT_DELAY)
    }
}

struct RawFrame {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

pub(super) struct RawAnimation {
    frames: Vec<RawFrame>,
    delays: Vec<Duration>,
    bytes: usize,
}

impl RawAnimation {
    fn into_animation(self) -> Animation {
        let Self {
            frames: raw,
            delays,
            bytes,
        } = self;
        let mut frames = Vec::with_capacity(raw.len());
        for frame in raw {
            frames.push(image_from_rgba(&frame.rgba, frame.width, frame.height));
        }
        Animation {
            frames,
            delays,
            bytes,
        }
    }
}

struct Playback {
    path: PathBuf,
    frame: usize,
    next_at: Instant,
    row_hint: usize,
}

struct DueFrame {
    unique_id: String,
    image: Image,
    row_hint: usize,
}

fn frame_delay(declared: Duration) -> Duration {
    if declared <= GIF_INSTANT_DELAY {
        GIF_DEFAULT_DELAY
    } else {
        declared
    }
}

enum AnimatedFormat {
    Gif,
    WebP,
}

fn animated_format(path: &Path) -> Option<AnimatedFormat> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "gif" => Some(AnimatedFormat::Gif),
        "webp" => Some(AnimatedFormat::WebP),
        _ => None,
    }
}

pub(super) fn is_animatable(path: &Path) -> bool {
    animated_format(path).is_some()
}

fn animation_limits() -> image::Limits {
    let mut limits = image::Limits::no_limits();
    limits.max_image_width = Some(ANIM_MAX_DIMENSION);
    limits.max_image_height = Some(ANIM_MAX_DIMENSION);
    limits.max_alloc = Some(ANIM_MAX_ALLOC);
    limits
}

fn bounded_frames<'a, D>(mut decoder: D) -> Option<Frames<'a>>
where
    D: ImageDecoder + AnimationDecoder<'a>,
{
    let (width, height) = decoder.dimensions();
    if width > ANIM_MAX_DIMENSION || height > ANIM_MAX_DIMENSION {
        return None;
    }
    decoder.set_limits(animation_limits()).ok()?;
    Some(decoder.into_frames())
}

fn frames_of(path: &Path) -> Option<Frames<'static>> {
    let reader = BufReader::new(File::open(path).ok()?);
    match animated_format(path)? {
        AnimatedFormat::Gif => bounded_frames(GifDecoder::new(reader).ok()?),
        AnimatedFormat::WebP => bounded_frames(WebPDecoder::new(reader).ok()?),
    }
}

#[derive(Default)]
struct AnimationBudget {
    source_pixels: u64,
    retained_bytes: usize,
}

impl AnimationBudget {
    fn admit(&mut self, buffer: RgbaImage) -> Option<RawFrame> {
        let (source_width, source_height) = buffer.dimensions();
        if source_width > ANIM_MAX_DIMENSION || source_height > ANIM_MAX_DIMENSION {
            return None;
        }
        self.source_pixels = self
            .source_pixels
            .saturating_add(u64::from(source_width) * u64::from(source_height));
        if self.source_pixels > ANIM_MAX_SOURCE_PIXELS {
            return None;
        }

        let buffer =
            if source_width > DISPLAY_MAX_DIMENSION || source_height > DISPLAY_MAX_DIMENSION {
                DynamicImage::ImageRgba8(buffer)
                    .thumbnail(DISPLAY_MAX_DIMENSION, DISPLAY_MAX_DIMENSION)
                    .into_rgba8()
            } else {
                buffer
            };
        let (width, height) = buffer.dimensions();

        self.retained_bytes = self
            .retained_bytes
            .saturating_add(width as usize * height as usize * 4);
        if self.retained_bytes > ANIM_PER_ITEM_BUDGET {
            return None;
        }

        Some(RawFrame {
            rgba: buffer.into_raw(),
            width,
            height,
        })
    }
}

pub(super) fn decode_raw(path: &Path) -> Option<RawAnimation> {
    let mut frames = Vec::new();
    let mut delays = Vec::new();
    let mut budget = AnimationBudget::default();

    for frame in frames_of(path)? {
        if frames.len() >= ANIM_MAX_FRAMES {
            break;
        }
        let Ok(frame) = frame else { break };
        let delay = frame_delay(Duration::from(frame.delay()));
        let Some(raw) = budget.admit(frame.into_buffer()) else {
            tracing::debug!(
                "animation at {} exceeds the decode budget, showing a still",
                path.display()
            );
            return None;
        };

        frames.push(raw);
        delays.push(delay);
    }

    (frames.len() > 1).then_some(RawAnimation {
        frames,
        delays,
        bytes: budget.retained_bytes,
    })
}

pub(super) fn on_decoded(path: &Path, decoded: Option<RawAnimation>, epoch: u64) {
    if super::is_stale(epoch) {
        return;
    }
    let animation = ANIMATIONS.with_borrow_mut(|state| {
        let remaining = ANIMATION_MEMORY_BUDGET.saturating_sub(state.retained_bytes());
        let animation = decoded
            .filter(|raw| raw.bytes <= remaining)
            .map(|raw| Rc::new(raw.into_animation()));
        state.clips.insert(path.to_path_buf(), animation.clone());
        animation
    });
    let waiting = waiters::take_media(path);

    let Some(animation) = animation else {
        for unique_id in waiting.unique_ids() {
            waiters::enqueue_media(path, unique_id, Lane::Static);
        }
        return;
    };

    ANIMATIONS.with_borrow_mut(|state| {
        for unique_id in waiting.unique_ids() {
            if matches!(
                state.start_playback(unique_id, path, &animation),
                PlaybackStart::AtCapacity
            ) {
                break;
            }
        }
    });
    reschedule();

    let first = animation
        .frame(0)
        .map_or(DecodeOutcome::Failed, DecodeOutcome::Ready);
    waiting.notify(first);
}

pub fn load_thumbnail(path: &Path, playback_key: &str) -> Option<Image> {
    if !is_animatable(path) {
        return cache::request_thumbnail(path, playback_key);
    }
    let animation = match ANIMATIONS.with_borrow(|state| state.clips.get(path).cloned()) {
        Some(Some(animation)) => animation,
        Some(None) => return cache::request_thumbnail(path, playback_key),
        None => {
            waiters::enqueue_media(path, playback_key, Lane::Animation);
            return None;
        }
    };

    let (frame, is_new) = ANIMATIONS.with_borrow_mut(|state| {
        if let Some(playback) = state.playbacks.get(playback_key) {
            return (playback.frame, false);
        }
        let started = state.start_playback(playback_key, path, &animation);
        (0, matches!(started, PlaybackStart::Started))
    });

    if is_new {
        reschedule();
    }

    animation.frame(frame).cloned()
}

fn due_frames(now: Instant) -> Vec<DueFrame> {
    ANIMATIONS.with_borrow_mut(|state| {
        let AnimationState { clips, playbacks } = state;
        let mut due = Vec::new();
        for (unique_id, playback) in playbacks.iter_mut() {
            if playback.next_at > now {
                continue;
            }
            let Some(animation) = clips.get(&playback.path).and_then(Option::as_ref) else {
                continue;
            };
            playback.frame = (playback.frame + 1) % animation.frames.len();
            playback.next_at = now + animation.delay(playback.frame);
            if let Some(frame) = animation.frame(playback.frame) {
                due.push(DueFrame {
                    unique_id: unique_id.clone(),
                    image: frame.clone(),
                    row_hint: playback.row_hint,
                });
            }
        }
        due
    })
}

fn forget_playbacks(gone: &[String]) {
    if gone.is_empty() {
        return;
    }
    ANIMATIONS.with_borrow_mut(|state| {
        let AnimationState { clips, playbacks } = state;
        for unique_id in gone {
            playbacks.remove(unique_id);
        }
        let live_paths = playbacks
            .values()
            .map(|playback| &playback.path)
            .collect::<HashSet<&PathBuf>>();
        clips.retain(|path, _| live_paths.contains(path));
    });
}

pub fn advance_animations(place_frame: &mut dyn FnMut(&str, usize, Image) -> Option<usize>) {
    let due = due_frames(Instant::now());
    if due.is_empty() {
        return;
    }

    let mut located = Vec::new();
    let mut gone = Vec::new();
    for item in due {
        match place_frame(&item.unique_id, item.row_hint, item.image) {
            Some(row) => located.push((item.unique_id, row)),
            None => gone.push(item.unique_id),
        }
    }

    ANIMATIONS.with_borrow_mut(|state| {
        for (unique_id, row) in located {
            if let Some(playback) = state.playbacks.get_mut(&unique_id) {
                playback.row_hint = row;
            }
        }
    });
    forget_playbacks(&gone);
}

fn next_deadline() -> Option<Instant> {
    ANIMATIONS.with_borrow(|state| state.playbacks.values().map(|p| p.next_at).min())
}

fn reschedule() {
    let Some(deadline) = next_deadline() else {
        ANIMATION_TIMER.with(Timer::stop);
        return;
    };
    let delay = deadline.saturating_duration_since(Instant::now());
    ANIMATION_TIMER.with(|timer| {
        if timer.running() {
            timer.set_interval(delay);
        } else {
            timer.start(TimerMode::Repeated, delay, on_deadline);
        }
    });
}

fn on_deadline() {
    if let Some(tick) = ANIMATION_TICK_FN.with_borrow(Clone::clone) {
        tick();
    }
    reschedule();
}

pub fn set_animation_tick(tick: impl Fn() + 'static) {
    ANIMATION_TICK_FN.with_borrow_mut(|slot| *slot = Some(Rc::new(tick)));
}

pub(super) fn clear() {
    ANIMATIONS.with_borrow_mut(|state| *state = AnimationState::default());
    ANIMATION_TIMER.with(Timer::stop);
}
