use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, Frames};
use slint::{Image, Model, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel};

const IMAGE_CACHE_MAX_BYTES: usize = 128 * 1024 * 1024;

const ANIMATION_MEMORY_BUDGET: usize = 256 * 1024 * 1024;
const ANIM_PER_ITEM_BUDGET: usize = 64 * 1024 * 1024;
const ANIM_MAX_DIMENSION: u32 = 2048;
const ANIM_MAX_FRAMES: usize = 600;

const GIF_INSTANT_DELAY: Duration = Duration::from_millis(10);
const GIF_DEFAULT_DELAY: Duration = Duration::from_millis(100);

thread_local! {
    static IMAGE_CACHE: RefCell<ImageCache> = RefCell::new(ImageCache::new());
    static ANIMATION_CACHE: RefCell<HashMap<PathBuf, Option<Rc<Animation>>>> = RefCell::new(HashMap::new());
    static PLAYBACKS: RefCell<HashMap<String, Playback>> = RefCell::new(HashMap::new());
    static ANIMATION_TIMER: Timer = Timer::default();
    static ANIMATION_TICK_FN: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
    static DECODE_TX: RefCell<Option<mpsc::Sender<PathBuf>>> = const { RefCell::new(None) };
    static IN_FLIGHT: RefCell<HashMap<PathBuf, Vec<String>>> = RefCell::new(HashMap::new());
    static IMAGE_READY_FN: RefCell<Option<ImageReadyFn>> = const { RefCell::new(None) };
}

type ImageReadyFn = Rc<dyn Fn(&str, Option<&Image>)>;

struct CachedImage {
    image: Image,
    bytes: usize,
    tick: u64,
}

struct ImageCache {
    entries: HashMap<PathBuf, CachedImage>,
    total_bytes: usize,
    tick: u64,
}

impl ImageCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            total_bytes: 0,
            tick: 0,
        }
    }

    fn get(&mut self, path: &Path) -> Option<Image> {
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        let entry = self.entries.get_mut(path)?;
        entry.tick = tick;
        Some(entry.image.clone())
    }

    fn insert(&mut self, path: PathBuf, image: Image, bytes: usize) {
        self.tick = self.tick.wrapping_add(1);
        if let Some(previous) = self.entries.insert(
            path,
            CachedImage {
                image,
                bytes,
                tick: self.tick,
            },
        ) {
            self.total_bytes = self.total_bytes.saturating_sub(previous.bytes);
        }
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.evict_to_budget();
    }

    fn evict_to_budget(&mut self) {
        while self.total_bytes > IMAGE_CACHE_MAX_BYTES {
            let Some(victim) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.tick)
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            if let Some(entry) = self.entries.remove(&victim) {
                self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
            }
        }
    }
}

fn image_from_rgba(rgba: &[u8], width: u32, height: u32) -> Image {
    let pixels = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba, width, height);
    Image::from_rgba8(pixels)
}

fn decode_rgba(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    let decoded = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?
        .to_rgba8();
    let (width, height) = decoded.dimensions();
    Some((decoded.into_raw(), width, height))
}

pub fn load_image_cached(path: &Path) -> Option<Image> {
    if let Some(img) = IMAGE_CACHE.with_borrow_mut(|cache| cache.get(path)) {
        return Some(img);
    }
    let (bytes, width, height) = decode_rgba(path)?;
    let img = image_from_rgba(&bytes, width, height);
    IMAGE_CACHE.with_borrow_mut(|cache| cache.insert(path.to_path_buf(), img.clone(), bytes.len()));
    Some(img)
}

fn ensure_workers() {
    DECODE_TX.with_borrow_mut(|slot| {
        if slot.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel::<PathBuf>();
        if let Err(e) = thread::Builder::new()
            .name("u2dm-image-decode".to_owned())
            .spawn(move || decode_loop(&rx))
        {
            tracing::warn!("failed to spawn image decode thread: {e}");
            return;
        }
        *slot = Some(tx);
    });
}

fn decode_loop(rx: &mpsc::Receiver<PathBuf>) {
    while let Ok(path) = rx.recv() {
        let decoded = decode_rgba(&path);
        drop(slint::invoke_from_event_loop(move || match decoded {
            Some((bytes, width, height)) => on_thumbnail_decoded(&path, &bytes, width, height),
            None => on_thumbnail_ready(&path, None),
        }));
    }
}

fn on_thumbnail_decoded(path: &Path, bytes: &[u8], width: u32, height: u32) {
    let image = image_from_rgba(bytes, width, height);
    IMAGE_CACHE
        .with_borrow_mut(|cache| cache.insert(path.to_path_buf(), image.clone(), bytes.len()));
    on_thumbnail_ready(path, Some(&image));
}

fn on_thumbnail_ready(path: &Path, image: Option<&Image>) {
    let waiting = IN_FLIGHT.with_borrow_mut(|inflight| inflight.remove(path));
    let Some(unique_ids) = waiting else {
        return;
    };
    let Some(ready) = IMAGE_READY_FN.with_borrow(Clone::clone) else {
        return;
    };
    for unique_id in unique_ids {
        ready(&unique_id, image);
    }
}

fn enqueue_thumbnail(path: &Path, unique_id: &str) {
    let should_dispatch = IN_FLIGHT.with_borrow_mut(|inflight| {
        let is_new = !inflight.contains_key(path);
        let waiting = inflight.entry(path.to_path_buf()).or_default();
        if !waiting.iter().any(|id| id == unique_id) {
            waiting.push(unique_id.to_owned());
        }
        is_new
    });
    if should_dispatch {
        ensure_workers();
        DECODE_TX.with_borrow(|slot| {
            if let Some(tx) = slot.as_ref() {
                drop(tx.send(path.to_path_buf()));
            }
        });
    }
}

pub fn set_image_ready(ready: impl Fn(&str, Option<&Image>) + 'static) {
    IMAGE_READY_FN.with_borrow_mut(|slot| *slot = Some(Rc::new(ready)));
}

pub fn patch_rows<T: Clone + 'static>(
    model: &VecModel<T>,
    matches: impl Fn(&T) -> bool,
    apply: impl Fn(&mut T),
) {
    for row in 0..model.row_count() {
        let Some(entry) = model.row_data(row) else {
            continue;
        };
        if matches(&entry) {
            let mut updated = entry;
            apply(&mut updated);
            model.set_row_data(row, updated);
        }
    }
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

struct Playback {
    path: PathBuf,
    frame: usize,
    next_at: Instant,
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

fn is_animatable(path: &Path) -> bool {
    animated_format(path).is_some()
}

fn frames_of(path: &Path) -> Option<Frames<'static>> {
    let reader = BufReader::new(File::open(path).ok()?);
    match animated_format(path)? {
        AnimatedFormat::Gif => Some(GifDecoder::new(reader).ok()?.into_frames()),
        AnimatedFormat::WebP => Some(WebPDecoder::new(reader).ok()?.into_frames()),
    }
}

fn frame_to_image(frame: image::Frame) -> Image {
    let buffer = frame.into_buffer();
    let (width, height) = buffer.dimensions();
    image_from_rgba(buffer.as_raw(), width, height)
}

fn decode_animation(path: &Path, budget: usize) -> Option<Animation> {
    let mut frames = Vec::new();
    let mut delays = Vec::new();
    let mut bytes: usize = 0;

    for frame in frames_of(path)? {
        if frames.len() >= ANIM_MAX_FRAMES {
            break;
        }
        let Ok(frame) = frame else { break };
        let delay = frame_delay(Duration::from(frame.delay()));
        let image = frame_to_image(frame);

        let width = image.size().width;
        let height = image.size().height;
        if width > ANIM_MAX_DIMENSION || height > ANIM_MAX_DIMENSION {
            tracing::debug!(
                "animation at {} exceeds the {ANIM_MAX_DIMENSION}px dimension cap, showing a still",
                path.display()
            );
            return None;
        }

        bytes = bytes.saturating_add(width as usize * height as usize * 4);
        if bytes > budget {
            tracing::debug!(
                "animation at {} exceeds the animation budget, showing a still",
                path.display()
            );
            return None;
        }

        frames.push(image);
        delays.push(delay);
    }

    (frames.len() > 1).then_some(Animation {
        frames,
        delays,
        bytes,
    })
}

fn cached_animation_bytes() -> usize {
    ANIMATION_CACHE.with_borrow(|cache| {
        cache
            .values()
            .filter_map(Option::as_ref)
            .map(|animation| animation.bytes)
            .sum()
    })
}

fn animation_cached(path: &Path) -> Option<Rc<Animation>> {
    if let Some(cached) = ANIMATION_CACHE.with_borrow(|cache| cache.get(path).cloned()) {
        return cached;
    }
    let remaining = ANIMATION_MEMORY_BUDGET.saturating_sub(cached_animation_bytes());
    let budget = remaining.min(ANIM_PER_ITEM_BUDGET);
    let decoded = decode_animation(path, budget).map(Rc::new);
    ANIMATION_CACHE.with_borrow_mut(|cache| {
        cache.insert(path.to_path_buf(), decoded.clone());
    });
    decoded
}

pub fn load_thumbnail(path: &Path, playback_key: &str) -> Option<Image> {
    if !is_animatable(path) {
        return request_thumbnail(path, playback_key);
    }
    let Some(animation) = animation_cached(path) else {
        return request_thumbnail(path, playback_key);
    };

    let (frame, is_new) = PLAYBACKS.with_borrow_mut(|playbacks| {
        let mut is_new = false;
        let playback = playbacks.entry(playback_key.to_owned()).or_insert_with(|| {
            is_new = true;
            Playback {
                path: path.to_path_buf(),
                frame: 0,
                next_at: Instant::now() + animation.delay(0),
            }
        });
        (playback.frame, is_new)
    });

    if is_new {
        reschedule_animations();
    }

    animation.frame(frame).cloned()
}

fn request_thumbnail(path: &Path, unique_id: &str) -> Option<Image> {
    if let Some(img) = IMAGE_CACHE.with_borrow_mut(|cache| cache.get(path)) {
        return Some(img);
    }
    enqueue_thumbnail(path, unique_id);
    None
}

fn due_frames(now: Instant) -> Vec<(String, Image)> {
    PLAYBACKS.with_borrow_mut(|playbacks| {
        let mut due = Vec::new();
        for (event_id, playback) in playbacks.iter_mut() {
            if playback.next_at > now {
                continue;
            }
            let Some(animation) = animation_cached(&playback.path) else {
                continue;
            };
            playback.frame = (playback.frame + 1) % animation.frames.len();
            playback.next_at = now + animation.delay(playback.frame);
            if let Some(frame) = animation.frame(playback.frame) {
                due.push((event_id.clone(), frame.clone()));
            }
        }
        due
    })
}

fn forget_animations_outside(live_event_ids: &HashSet<String>) {
    let live_paths = PLAYBACKS.with_borrow_mut(|playbacks| {
        playbacks.retain(|event_id, _| live_event_ids.contains(event_id));
        playbacks
            .values()
            .map(|playback| playback.path.clone())
            .collect::<HashSet<PathBuf>>()
    });
    ANIMATION_CACHE.with_borrow_mut(|cache| cache.retain(|path, _| live_paths.contains(path)));
}

pub fn advance_animations<T: Clone + 'static>(
    timeline_model: &VecModel<T>,
    entry_id: &dyn Fn(&T) -> String,
    set_thumbnail: &dyn Fn(&mut T, Image),
) {
    let due = due_frames(Instant::now());
    if due.is_empty() {
        return;
    }

    let mut live_event_ids = HashSet::new();
    for row in 0..timeline_model.row_count() {
        let Some(entry) = timeline_model.row_data(row) else {
            continue;
        };
        let event_id = entry_id(&entry);
        if let Some((_, frame)) = due.iter().find(|(id, _)| *id == event_id) {
            let mut updated = entry;
            set_thumbnail(&mut updated, frame.clone());
            timeline_model.set_row_data(row, updated);
        }
        live_event_ids.insert(event_id);
    }

    forget_animations_outside(&live_event_ids);
}

fn next_deadline() -> Option<Instant> {
    PLAYBACKS.with_borrow(|playbacks| playbacks.values().map(|p| p.next_at).min())
}

fn reschedule_animations() {
    let Some(deadline) = next_deadline() else {
        ANIMATION_TIMER.with(Timer::stop);
        return;
    };
    let delay = deadline.saturating_duration_since(Instant::now());
    ANIMATION_TIMER.with(|timer| {
        if timer.running() {
            timer.set_interval(delay);
        } else {
            timer.start(TimerMode::Repeated, delay, on_animation_deadline);
        }
    });
}

fn on_animation_deadline() {
    if let Some(tick) = ANIMATION_TICK_FN.with_borrow(Clone::clone) {
        tick();
    }
    reschedule_animations();
}

pub fn set_animation_tick(tick: impl Fn() + 'static) {
    ANIMATION_TICK_FN.with_borrow_mut(|slot| *slot = Some(Rc::new(tick)));
}
