use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::mem;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gif::{DecodeOptions, Repeat};
use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::metadata::LoopCount;
use image::{AnimationDecoder, DynamicImage, Frames, ImageDecoder, RgbaImage};
use slint::{Image, Timer, TimerMode};

use super::cache::{DecodeFailure, Decoded};
use super::requests::Needs;
use super::slots::{MediaSlot, Surface};
use super::waiters::DecodeOutcome;
use super::workers::Lane;
use super::{
    DISPLAY_MAX_DIMENSION, Epoch, MediaSession, cache, image_from_rgba, waiters, with_media,
};

const ANIMATION_MEMORY_BUDGET: usize = 128 * 1024 * 1024;
const ANIM_PER_ITEM_BUDGET: usize = 64 * 1024 * 1024;
const ANIM_MAX_DIMENSION: u32 = 2048;
const ANIM_MAX_FRAMES: usize = 600;
const ANIM_MAX_SOURCE_FRAMES: usize = 2400;
const ANIM_MAX_SOURCE_PIXELS: u64 = 128 * 1024 * 1024;
const ANIM_SMOOTH_DELAY: Duration = Duration::from_millis(50);
const ANIM_SHRINK_PERCENT: u32 = 70;
const ANIM_MIN_DIMENSION: u32 = 128;
const ANIM_CANVAS_BYTES: u64 = 4 * ANIM_MAX_DIMENSION as u64 * ANIM_MAX_DIMENSION as u64;
const ANIM_CONCURRENT_CANVASES: u64 = 4;
const ANIM_MAX_ALLOC: u64 = ANIM_CONCURRENT_CANVASES * ANIM_CANVAS_BYTES;
const MAX_ACTIVE_ANIMATIONS: usize = 16;

const GIF_INSTANT_DELAY: Duration = Duration::from_millis(10);
const GIF_DEFAULT_DELAY: Duration = Duration::from_millis(100);

thread_local! {
    static ANIMATION_TIMER: Timer = Timer::default();
    static ANIMATION_TICK_FN: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
    static ANIMATIONS_PAUSED: Cell<bool> = const { Cell::new(false) };
}

#[derive(Default)]
pub(super) struct AnimationState {
    clips: HashMap<PathBuf, Clip>,
    playbacks: HashMap<MediaSlot, Playback>,
    pngs: HashMap<PathBuf, PngKind>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PngKind {
    Still,
    Animated,
}

#[derive(Clone)]
enum Clip {
    Animated(Rc<Animation>),
    Still,
    OverBudget(Surface),
}

impl Clip {
    fn animation(&self) -> Option<&Rc<Animation>> {
        match self {
            Self::Animated(animation) => Some(animation),
            Self::Still | Self::OverBudget(_) => None,
        }
    }
}

pub(super) enum Peek {
    Frame(Image),
    Still,
    Undecided,
}

fn with_animations<R>(f: impl FnOnce(&mut AnimationState) -> R) -> R {
    with_media(|media| f(&mut media.animations))
}

fn with_animations_and_needs<R>(f: impl FnOnce(&mut AnimationState, &Needs) -> R) -> R {
    with_media(|media| {
        let MediaSession {
            animations, needs, ..
        } = media;
        f(animations, needs)
    })
}

impl AnimationState {
    fn retained_bytes(&self, surface: Surface) -> usize {
        self.clips
            .values()
            .filter_map(Clip::animation)
            .filter(|animation| animation.surface == surface)
            .map(|animation| animation.bytes)
            .sum()
    }

    fn start_playback(
        &mut self,
        needs: &Needs,
        slot: &MediaSlot,
        path: &Path,
        animation: &Animation,
    ) -> PlaybackStart {
        if self.playing(slot, path).is_some() {
            return PlaybackStart::AlreadyRunning;
        }
        let others_playing = self
            .playbacks
            .iter()
            .filter(|&(other, playback)| {
                other != slot
                    && other.surface() == slot.surface()
                    && playback.plays_on()
                    && needs.expects_media(other, &playback.path)
            })
            .count();
        if others_playing >= MAX_ACTIVE_ANIMATIONS {
            return PlaybackStart::AtCapacity;
        }
        self.playbacks.insert(
            slot.clone(),
            Playback {
                path: path.to_path_buf(),
                frame: 0,
                completed_plays: 0,
                next: NextFrame::At(Instant::now() + animation.delay(0)),
                row_hint: 0,
            },
        );
        PlaybackStart::Started
    }

    fn playing(&self, slot: &MediaSlot, path: &Path) -> Option<&Playback> {
        self.playbacks
            .get(slot)
            .filter(|playback| playback.path == path)
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
    plays: Plays,
    bytes: usize,
    surface: Surface,
}

#[derive(Clone, Copy)]
enum Plays {
    Forever,
    Times(NonZeroU32),
}

impl Plays {
    fn of_gif(repeat: Repeat) -> Self {
        match repeat {
            Repeat::Infinite => Self::Forever,
            Repeat::Finite(repeats) => {
                Self::Times(NonZeroU32::MIN.saturating_add(u32::from(repeats)))
            }
        }
    }

    fn of_loop_count(count: LoopCount) -> Self {
        match count {
            LoopCount::Infinite => Self::Forever,
            LoopCount::Finite(times) => Self::Times(times),
        }
    }

    fn ends_after(self, completed: u32) -> bool {
        match self {
            Self::Forever => false,
            Self::Times(times) => completed >= times.get(),
        }
    }
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

impl RawFrame {
    fn fitted(buffer: RgbaImage, max_dimension: u32) -> Self {
        let (width, height) = buffer.dimensions();
        let buffer = if width > max_dimension || height > max_dimension {
            DynamicImage::ImageRgba8(buffer)
                .thumbnail(max_dimension, max_dimension)
                .into_rgba8()
        } else {
            buffer
        };
        let (width, height) = buffer.dimensions();
        Self {
            rgba: buffer.into_raw(),
            width,
            height,
        }
    }

    fn largest_side(&self) -> u32 {
        self.width.max(self.height)
    }

    fn shrunk(self, max_dimension: u32) -> Option<Self> {
        RgbaImage::from_raw(self.width, self.height, self.rgba)
            .map(|buffer| Self::fitted(buffer, max_dimension))
    }
}

pub(super) struct RawAnimation {
    frames: Vec<RawFrame>,
    delays: Vec<Duration>,
    plays: Plays,
    bytes: usize,
}

impl RawAnimation {
    fn into_animation(self, surface: Surface) -> Animation {
        let Self {
            frames: raw,
            delays,
            plays,
            bytes,
        } = self;
        let mut frames = Vec::with_capacity(raw.len());
        for frame in raw {
            frames.push(image_from_rgba(&frame.rgba, frame.width, frame.height));
        }
        Animation {
            frames,
            delays,
            plays,
            bytes,
            surface,
        }
    }
}

struct Playback {
    path: PathBuf,
    frame: usize,
    completed_plays: u32,
    next: NextFrame,
    row_hint: usize,
}

#[derive(Clone, Copy)]
enum NextFrame {
    At(Instant),
    Ended,
}

enum Step {
    Frame,
    LastPlayEnded,
}

impl Playback {
    fn due_at(&self) -> Option<Instant> {
        match self.next {
            NextFrame::At(at) => Some(at),
            NextFrame::Ended => None,
        }
    }

    fn plays_on(&self) -> bool {
        self.due_at().is_some()
    }

    fn is_due(&self, now: Instant) -> bool {
        self.due_at().is_some_and(|at| at <= now)
    }

    fn step(&mut self, animation: &Animation, now: Instant) -> Step {
        let next = self.frame + 1;
        if next < animation.frames.len() {
            self.frame = next;
        } else {
            self.completed_plays = self.completed_plays.saturating_add(1);
            if animation.plays.ends_after(self.completed_plays) {
                self.next = NextFrame::Ended;
                return Step::LastPlayEnded;
            }
            self.frame = 0;
        }
        self.next = NextFrame::At(now + animation.delay(self.frame));
        Step::Frame
    }
}

struct DueFrame {
    slot: MediaSlot,
    image: Image,
    row_hint: usize,
}

#[derive(Default)]
struct Tick {
    due: Vec<DueFrame>,
    unexpected: Vec<MediaSlot>,
}

fn frame_delay(declared: Duration) -> Duration {
    if declared <= GIF_INSTANT_DELAY {
        GIF_DEFAULT_DELAY
    } else {
        declared
    }
}

#[derive(Clone, Copy)]
enum AnimatableFormat {
    Gif,
    WebP,
    Png,
}

fn animatable_format(path: &Path) -> Option<AnimatableFormat> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "gif" => Some(AnimatableFormat::Gif),
        "webp" => Some(AnimatableFormat::WebP),
        "png" | "apng" => Some(AnimatableFormat::Png),
        _ => None,
    }
}

pub fn is_animatable(path: &Path) -> bool {
    match animatable_format(path) {
        Some(AnimatableFormat::Gif | AnimatableFormat::WebP) => true,
        Some(AnimatableFormat::Png) => png_kind(path) == PngKind::Animated,
        None => false,
    }
}

fn png_kind(path: &Path) -> PngKind {
    if let Some(known) = with_animations(|state| state.pngs.get(path).copied()) {
        return known;
    }
    let Some(found) = read_png_kind(path) else {
        return PngKind::Still;
    };
    with_animations(|state| state.pngs.insert(path.to_path_buf(), found));
    found
}

fn read_png_kind(path: &Path) -> Option<PngKind> {
    let decoder = PngDecoder::new(BufReader::new(File::open(path).ok()?)).ok()?;
    if decoder.is_apng().ok()? {
        Some(PngKind::Animated)
    } else {
        Some(PngKind::Still)
    }
}

fn plays(path: &Path) -> bool {
    is_animatable(path) && !ANIMATIONS_PAUSED.get()
}

pub fn set_animations_paused(paused: bool) {
    let was_paused = ANIMATIONS_PAUSED.replace(paused);
    if paused && !was_paused && slint::invoke_from_event_loop(on_deadline).is_err() {
        on_deadline();
    }
}

fn animation_limits() -> image::Limits {
    let mut limits = image::Limits::no_limits();
    limits.max_image_width = Some(ANIM_MAX_DIMENSION);
    limits.max_image_height = Some(ANIM_MAX_DIMENSION);
    limits.max_alloc = Some(ANIM_MAX_ALLOC);
    limits
}

fn bounded<D: ImageDecoder>(mut decoder: D) -> Option<D> {
    let (width, height) = decoder.dimensions();
    if width > ANIM_MAX_DIMENSION || height > ANIM_MAX_DIMENSION {
        return None;
    }
    decoder.set_limits(animation_limits()).ok()?;
    Some(decoder)
}

fn frames_and_plays(path: &Path) -> Option<(Frames<'static>, Plays)> {
    let reader = BufReader::new(File::open(path).ok()?);
    match animatable_format(path)? {
        AnimatableFormat::Gif => {
            let plays = gif_plays(path)?;
            let decoder = bounded(GifDecoder::new(reader).ok()?)?;
            Some((decoder.into_frames(), plays))
        }
        AnimatableFormat::WebP => {
            let decoder = bounded(WebPDecoder::new(reader).ok()?)?;
            let plays = Plays::of_loop_count(decoder.loop_count());
            Some((decoder.into_frames(), plays))
        }
        AnimatableFormat::Png => {
            let decoder = bounded(PngDecoder::new(reader).ok()?)?;
            if !decoder.is_apng().ok()? {
                return None;
            }
            let decoder = decoder.apng().ok()?;
            let plays = Plays::of_loop_count(decoder.loop_count());
            Some((decoder.into_frames(), plays))
        }
    }
}

fn gif_plays(path: &Path) -> Option<Plays> {
    let reader = BufReader::new(File::open(path).ok()?);
    let header = DecodeOptions::new().read_info(reader).ok()?;
    Some(Plays::of_gif(header.repeat()))
}

struct Reel {
    frames: Vec<RawFrame>,
    delays: Vec<Duration>,
    stride: usize,
    max_dimension: u32,
    source_pixels: u64,
}

impl Reel {
    fn new() -> Self {
        Self {
            frames: Vec::new(),
            delays: Vec::new(),
            stride: 1,
            max_dimension: DISPLAY_MAX_DIMENSION,
            source_pixels: 0,
        }
    }

    fn bytes(&self) -> usize {
        self.frames.iter().map(|frame| frame.rgba.len()).sum()
    }

    fn fits(&self) -> bool {
        self.frames.len() <= ANIM_MAX_FRAMES && self.bytes() <= ANIM_PER_ITEM_BUDGET
    }

    fn take(&mut self, index: usize, buffer: RgbaImage, delay: Duration) -> Option<()> {
        let (width, height) = buffer.dimensions();
        if width > ANIM_MAX_DIMENSION || height > ANIM_MAX_DIMENSION {
            return None;
        }
        self.source_pixels = self
            .source_pixels
            .saturating_add(u64::from(width) * u64::from(height));
        if self.source_pixels > ANIM_MAX_SOURCE_PIXELS {
            return None;
        }
        if !index.is_multiple_of(self.stride) {
            if let Some(last) = self.delays.last_mut() {
                *last += delay;
            }
            return Some(());
        }
        self.frames
            .push(RawFrame::fitted(buffer, self.max_dimension));
        self.delays.push(delay);
        while !self.fits() {
            self.reduce()?;
        }
        Some(())
    }

    fn reduce(&mut self) -> Option<()> {
        let smooth = self
            .delays
            .split_last()
            .and_then(|(_, settled)| settled.iter().min())
            .is_some_and(|delay| *delay < ANIM_SMOOTH_DELAY);
        if smooth || self.frames.len() > ANIM_MAX_FRAMES {
            self.halve_frame_rate();
            return Some(());
        }
        let largest = self.frames.iter().map(RawFrame::largest_side).max()?;
        self.max_dimension = largest * ANIM_SHRINK_PERCENT / 100;
        if self.max_dimension < ANIM_MIN_DIMENSION {
            return None;
        }
        let max_dimension = self.max_dimension;
        self.frames = mem::take(&mut self.frames)
            .into_iter()
            .map(|frame| frame.shrunk(max_dimension))
            .collect::<Option<Vec<_>>>()?;
        Some(())
    }

    fn halve_frame_rate(&mut self) {
        let frames = mem::take(&mut self.frames);
        let delays = mem::take(&mut self.delays);
        for (index, (frame, delay)) in frames.into_iter().zip(delays).enumerate() {
            if index % 2 == 0 {
                self.frames.push(frame);
                self.delays.push(delay);
            } else if let Some(last) = self.delays.last_mut() {
                *last += delay;
            }
        }
        self.stride *= 2;
    }

    fn into_raw(self, plays: Plays) -> Option<RawAnimation> {
        let bytes = self.bytes();
        (self.frames.len() > 1).then_some(RawAnimation {
            frames: self.frames,
            delays: self.delays,
            plays,
            bytes,
        })
    }
}

pub(super) fn decode_raw(path: &Path) -> Option<RawAnimation> {
    let (frames, plays) = frames_and_plays(path)?;
    let mut reel = Reel::new();
    for (index, frame) in frames.enumerate().take(ANIM_MAX_SOURCE_FRAMES) {
        let Ok(frame) = frame else { break };
        let delay = frame_delay(Duration::from(frame.delay()));
        if reel.take(index, frame.into_buffer(), delay).is_none() {
            tracing::debug!(
                "animation at {} exceeds the decode budget, showing a still",
                path.display()
            );
            return None;
        }
    }
    if reel.stride > 1 || reel.max_dimension < DISPLAY_MAX_DIMENSION {
        tracing::debug!(
            stride = reel.stride,
            max_dimension = reel.max_dimension,
            "animation at {} reduced to fit the decode budget",
            path.display()
        );
    }
    reel.into_raw(plays)
}

fn surface_paying_for(slots: &[MediaSlot]) -> Surface {
    if slots.iter().any(MediaSlot::belongs_to_timeline) {
        Surface::Timeline
    } else {
        Surface::Picker
    }
}

pub(super) fn on_decoded(path: &Path, decoded: Option<RawAnimation>, epoch: Epoch) {
    if !epoch.is_current() {
        return;
    }
    let waiting = waiters::take_media(path);
    if ANIMATIONS_PAUSED.get() {
        for slot in waiting.media_slots() {
            waiters::enqueue_media(path, slot, Lane::Static);
        }
        return;
    }
    let surface = surface_paying_for(waiting.media_slots());
    let clip = with_animations(|state| {
        let remaining = ANIMATION_MEMORY_BUDGET.saturating_sub(state.retained_bytes(surface));
        let clip = match decoded {
            None => Clip::Still,
            Some(raw) if raw.bytes <= remaining => {
                Clip::Animated(Rc::new(raw.into_animation(surface)))
            }
            Some(_) => Clip::OverBudget(surface),
        };
        state.clips.insert(path.to_path_buf(), clip.clone());
        clip
    });

    let Clip::Animated(animation) = clip else {
        for slot in waiting.media_slots() {
            waiters::enqueue_media(path, slot, Lane::Static);
        }
        return;
    };

    with_animations_and_needs(|state, needs| {
        for slot in waiting.media_slots() {
            state.start_playback(needs, slot, path, &animation);
        }
    });
    reschedule();

    let first = animation.frame(0).map_or(
        DecodeOutcome::Failed(DecodeFailure::Unreadable),
        DecodeOutcome::Ready,
    );
    waiting.notify(first);
}

pub(super) fn peek(path: &Path, slot: &MediaSlot) -> Peek {
    if !plays(path) {
        return Peek::Still;
    }
    with_animations(|state| match state.clips.get(path) {
        Some(Clip::Animated(animation)) => state
            .playing(slot, path)
            .and_then(|playback| animation.frame(playback.frame).cloned())
            .map_or(Peek::Undecided, Peek::Frame),
        Some(Clip::Still) => Peek::Still,
        Some(Clip::OverBudget(surface)) if *surface == slot.surface() => Peek::Still,
        Some(Clip::OverBudget(_)) | None => Peek::Undecided,
    })
}

pub fn load_thumbnail(path: &Path, slot: &MediaSlot) -> Decoded {
    with_media(|media| media.needs.expect_media(slot, Some(path)));
    if !plays(path) {
        return cache::request_thumbnail(path, slot);
    }
    let animation = match with_animations(|state| state.clips.get(path).cloned()) {
        Some(Clip::Animated(animation)) => animation,
        Some(Clip::Still) => return cache::request_thumbnail(path, slot),
        Some(Clip::OverBudget(surface)) if surface == slot.surface() => {
            return cache::request_thumbnail(path, slot);
        }
        Some(Clip::OverBudget(_)) | None => {
            waiters::enqueue_media(path, slot, Lane::Animation);
            return Decoded::Pending;
        }
    };

    let (frame, is_new) = with_animations_and_needs(|state, needs| {
        if let Some(playback) = state.playing(slot, path) {
            return (playback.frame, false);
        }
        let started = state.start_playback(needs, slot, path, &animation);
        (0, matches!(started, PlaybackStart::Started))
    });

    if is_new {
        reschedule();
    }

    animation
        .frame(frame)
        .cloned()
        .map_or(Decoded::Failed(DecodeFailure::Unreadable), Decoded::Ready)
}

fn due_frames(now: Instant) -> Tick {
    with_animations_and_needs(|state, needs| {
        let AnimationState {
            clips, playbacks, ..
        } = state;
        let paused = ANIMATIONS_PAUSED.get();
        let mut tick = Tick::default();
        for (slot, playback) in playbacks.iter_mut() {
            if !needs.expects_media(slot, &playback.path) {
                tick.unexpected.push(slot.clone());
                continue;
            }
            if !paused && !playback.is_due(now) {
                continue;
            }
            let Some(animation) = clips.get(&playback.path).and_then(Clip::animation) else {
                continue;
            };
            if paused {
                tick.unexpected.push(slot.clone());
                playback.frame = 0;
            } else if let Step::LastPlayEnded = playback.step(animation, now) {
                continue;
            }
            if let Some(frame) = animation.frame(playback.frame) {
                tick.due.push(DueFrame {
                    slot: slot.clone(),
                    image: frame.clone(),
                    row_hint: playback.row_hint,
                });
            }
        }
        tick
    })
}

fn forget_playbacks(gone: &[MediaSlot]) {
    if gone.is_empty() {
        return;
    }
    with_animations(|state| {
        let AnimationState {
            clips, playbacks, ..
        } = state;
        for slot in gone {
            playbacks.remove(slot);
        }
        let live_paths = playbacks
            .values()
            .map(|playback| &playback.path)
            .collect::<HashSet<&PathBuf>>();
        let mut freed = HashSet::new();
        clips.retain(|path, clip| match clip {
            Clip::Animated(animation) if !live_paths.contains(path) => {
                freed.insert(animation.surface);
                false
            }
            Clip::Animated(_) | Clip::Still | Clip::OverBudget(_) => true,
        });
        clips.retain(
            |_, clip| !matches!(clip, Clip::OverBudget(surface) if freed.contains(surface)),
        );
    });
}

pub fn advance_animations(place_frame: &mut dyn FnMut(&MediaSlot, usize, Image) -> Option<usize>) {
    let Tick {
        due,
        unexpected: mut gone,
    } = due_frames(Instant::now());

    let mut located = Vec::new();
    for item in due {
        match place_frame(&item.slot, item.row_hint, item.image) {
            Some(row) => located.push((item.slot, row)),
            None => gone.push(item.slot),
        }
    }

    with_animations(|state| {
        for (slot, row) in located {
            if let Some(playback) = state.playbacks.get_mut(&slot) {
                playback.row_hint = row;
            }
        }
    });
    forget_playbacks(&gone);
}

fn next_deadline() -> Option<Instant> {
    with_animations(|state| state.playbacks.values().filter_map(Playback::due_at).min())
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
