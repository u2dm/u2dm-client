use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use std::{slice, thread};

use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, DynamicImage, Frames};
use slint::{Image, Model, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel};

const IMAGE_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;

const ANIMATION_MEMORY_BUDGET: usize = 128 * 1024 * 1024;
const ANIM_PER_ITEM_BUDGET: usize = 32 * 1024 * 1024;
const ANIM_MAX_DIMENSION: u32 = 2048;
const ANIM_MAX_FRAMES: usize = 600;
const MAX_ACTIVE_ANIMATIONS: usize = 16;

const DECODE_MAX_DIMENSION: u32 = 4096;
const DECODE_MAX_ALLOC: u64 = 4 * DECODE_MAX_DIMENSION as u64 * DECODE_MAX_DIMENSION as u64;
const DISPLAY_MAX_DIMENSION: u32 = 512;

const DECODE_LANE_CAP: usize = 1024;
const MAX_DECODE_WORKERS: usize = 3;

const GIF_INSTANT_DELAY: Duration = Duration::from_millis(10);
const GIF_DEFAULT_DELAY: Duration = Duration::from_millis(100);

thread_local! {
    static IMAGE_CACHE: RefCell<ImageCache> = RefCell::new(ImageCache::new());
    static ANIMATION_CACHE: RefCell<HashMap<PathBuf, Option<Rc<Animation>>>> = RefCell::new(HashMap::new());
    static PLAYBACKS: RefCell<HashMap<String, Playback>> = RefCell::new(HashMap::new());
    static ANIMATION_TIMER: Timer = Timer::default();
    static ANIMATION_TICK_FN: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
    static DECODE_QUEUE: RefCell<Option<Arc<DecodeQueue>>> = const { RefCell::new(None) };
    static IN_FLIGHT: RefCell<HashMap<PathBuf, Vec<String>>> = RefCell::new(HashMap::new());
    static AVATAR_WAITERS: RefCell<HashMap<PathBuf, Vec<AvatarSlot>>> = RefCell::new(HashMap::new());
    static MEDIA_NEEDS: RefCell<HashMap<String, MediaNeed>> = RefCell::new(HashMap::new());
    static AVATAR_NEEDS: RefCell<HashMap<AvatarSlot, PathBuf>> = RefCell::new(HashMap::new());
    static IMAGE_READY_FN: RefCell<Option<ImageReadyFn>> = const { RefCell::new(None) };
    static AVATAR_READY_FN: RefCell<Option<AvatarReadyFn>> = const { RefCell::new(None) };
}

type ImageReadyFn = Rc<dyn Fn(&str, DecodeOutcome<'_>)>;
type AvatarReadyFn = Rc<dyn Fn(&[AvatarSlot], DecodeOutcome<'_>)>;

#[derive(Clone, Copy)]
pub enum DecodeOutcome<'a> {
    Ready(&'a Image),
    Failed,
    Deferred,
}

/// Identifies which UI row an off-thread avatar decode should patch once it lands.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum AvatarSlot {
    Message(String),
    Room(String),
    Space(String),
    User,
}

#[derive(Clone)]
struct MediaNeed {
    thumbnail: Option<PathBuf>,
    avatar: Option<PathBuf>,
}

enum Lookup {
    Hit(Image),
    Failed,
    Miss,
}

struct CachedImage {
    image: Option<Image>,
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

    fn lookup(&mut self, path: &Path) -> Lookup {
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        let Some(entry) = self.entries.get_mut(path) else {
            return Lookup::Miss;
        };
        entry.tick = tick;
        match &entry.image {
            Some(image) => Lookup::Hit(image.clone()),
            None => Lookup::Failed,
        }
    }

    fn insert(&mut self, path: PathBuf, image: Option<Image>, bytes: usize) {
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

fn decode_limits() -> image::Limits {
    let mut limits = image::Limits::no_limits();
    limits.max_image_width = Some(DECODE_MAX_DIMENSION);
    limits.max_image_height = Some(DECODE_MAX_DIMENSION);
    limits.max_alloc = Some(DECODE_MAX_ALLOC);
    limits
}

fn decode_rgba(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    let mut reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    reader.limits(decode_limits());
    let decoded = reader.decode().ok()?;

    let decoded =
        if decoded.width() > DISPLAY_MAX_DIMENSION || decoded.height() > DISPLAY_MAX_DIMENSION {
            decoded.thumbnail(DISPLAY_MAX_DIMENSION, DISPLAY_MAX_DIMENSION)
        } else {
            decoded
        };

    let rgba = decoded.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))?;
    let raw = rgba.into_raw();
    (raw.len() == expected_len).then_some((raw, width, height))
}

#[derive(Clone, Copy)]
enum Lane {
    Avatar,
    Static,
    Animation,
}

struct QueueInner {
    avatar: VecDeque<PathBuf>,
    static_img: VecDeque<PathBuf>,
    animation: VecDeque<PathBuf>,
}

impl QueueInner {
    fn new() -> Self {
        Self {
            avatar: VecDeque::new(),
            static_img: VecDeque::new(),
            animation: VecDeque::new(),
        }
    }

    fn lane_mut(&mut self, lane: Lane) -> &mut VecDeque<PathBuf> {
        match lane {
            Lane::Avatar => &mut self.avatar,
            Lane::Static => &mut self.static_img,
            Lane::Animation => &mut self.animation,
        }
    }

    fn take_front(&mut self) -> Option<(Lane, PathBuf)> {
        for lane in [Lane::Avatar, Lane::Static, Lane::Animation] {
            if let Some(path) = self.lane_mut(lane).pop_front() {
                return Some((lane, path));
            }
        }
        None
    }

    fn push_back_bounded(&mut self, lane: Lane, path: PathBuf) -> Option<PathBuf> {
        let queue = self.lane_mut(lane);
        queue.push_back(path);
        (queue.len() > DECODE_LANE_CAP)
            .then(|| queue.pop_front())
            .flatten()
    }
}

struct DecodeQueue {
    inner: Mutex<QueueInner>,
    signal: Condvar,
}

fn lock(mutex: &Mutex<QueueInner>) -> MutexGuard<'_, QueueInner> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn ensure_workers() {
    DECODE_QUEUE.with_borrow_mut(|slot| {
        if slot.is_some() {
            return;
        }
        let queue = Arc::new(DecodeQueue {
            inner: Mutex::new(QueueInner::new()),
            signal: Condvar::new(),
        });
        let worker_count = thread::available_parallelism()
            .map_or(2, |n| n.get().saturating_sub(1))
            .clamp(1, MAX_DECODE_WORKERS);
        let mut spawned = 0;
        for index in 0..worker_count {
            let queue = Arc::clone(&queue);
            match thread::Builder::new()
                .name(format!("u2dm-image-decode-{index}"))
                .spawn(move || decode_worker(&queue))
            {
                Ok(_) => spawned += 1,
                Err(e) => tracing::warn!("failed to spawn image decode thread: {e}"),
            }
        }
        if spawned > 0 {
            *slot = Some(queue);
        }
    });
}

fn decode_worker(queue: &Arc<DecodeQueue>) {
    loop {
        let (lane, path) = next_job(queue);
        run_job(lane, &path);
    }
}

fn next_job(queue: &Arc<DecodeQueue>) -> (Lane, PathBuf) {
    let mut inner = lock(&queue.inner);
    loop {
        if let Some(picked) = inner.take_front() {
            return picked;
        }
        inner = queue
            .signal
            .wait(inner)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

fn run_job(lane: Lane, path: &Path) {
    let path = path.to_path_buf();
    match lane {
        Lane::Avatar | Lane::Static => {
            let decoded = decode_rgba(&path);
            drop(slint::invoke_from_event_loop(move || {
                on_static_decoded(&path, decoded);
            }));
        }
        Lane::Animation => {
            let decoded = decode_raw_animation(&path);
            drop(slint::invoke_from_event_loop(move || {
                on_animation_decoded(&path, decoded);
            }));
        }
    }
}

fn on_static_decoded(path: &Path, decoded: Option<(Vec<u8>, u32, u32)>) {
    let decoded = decoded.map(|(bytes, width, height)| {
        let len = bytes.len();
        (image_from_rgba(&bytes, width, height), len)
    });
    let bytes = decoded.as_ref().map_or(0, |(_, len)| *len);
    let image = decoded.map(|(image, _)| image);
    IMAGE_CACHE.with_borrow_mut(|cache| cache.insert(path.to_path_buf(), image.clone(), bytes));

    let outcome = image
        .as_ref()
        .map_or(DecodeOutcome::Failed, DecodeOutcome::Ready);
    if let Some(unique_ids) = IN_FLIGHT.with_borrow_mut(|inflight| inflight.remove(path)) {
        notify_ready(&unique_ids, outcome);
    }
    if let Some(slots) = AVATAR_WAITERS.with_borrow_mut(|waiters| waiters.remove(path)) {
        notify_avatar_ready(&slots, outcome);
    }
}

fn notify_ready(unique_ids: &[String], outcome: DecodeOutcome<'_>) {
    let Some(ready) = IMAGE_READY_FN.with_borrow(Clone::clone) else {
        return;
    };
    for unique_id in unique_ids {
        ready(unique_id, outcome);
    }
}

fn notify_avatar_ready(slots: &[AvatarSlot], outcome: DecodeOutcome<'_>) {
    if let Some(ready) = AVATAR_READY_FN.with_borrow(Clone::clone) {
        ready(slots, outcome);
    }
}

fn send_job(lane: Lane, path: PathBuf) {
    ensure_workers();
    let dropped = DECODE_QUEUE.with_borrow(|slot| {
        let queue = slot.as_ref()?;
        let dropped = {
            let mut inner = lock(&queue.inner);
            inner.push_back_bounded(lane, path)
        };
        queue.signal.notify_all();
        dropped
    });
    if let Some(dropped) = dropped {
        defer_dropped(lane, &dropped);
    }
}

fn defer_dropped(lane: Lane, path: &Path) {
    tracing::warn!(
        "decode lane at capacity, deferred {}; it will be re-requested",
        path.display()
    );
    match lane {
        Lane::Avatar => {
            if let Some(slots) = AVATAR_WAITERS.with_borrow_mut(|waiters| waiters.remove(path)) {
                notify_avatar_ready(&slots, DecodeOutcome::Deferred);
            }
        }
        Lane::Static | Lane::Animation => {
            if let Some(unique_ids) = IN_FLIGHT.with_borrow_mut(|inflight| inflight.remove(path)) {
                notify_ready(&unique_ids, DecodeOutcome::Deferred);
            }
        }
    }
}

fn enqueue_decode(path: &Path, unique_id: &str, lane: Lane) {
    let should_dispatch = IN_FLIGHT.with_borrow_mut(|inflight| {
        let is_new = !inflight.contains_key(path);
        let waiting = inflight.entry(path.to_path_buf()).or_default();
        if !waiting.iter().any(|id| id == unique_id) {
            waiting.push(unique_id.to_owned());
        }
        is_new
    });
    if should_dispatch {
        send_job(lane, path.to_path_buf());
    }
}

pub fn load_avatar_async(path: &Path, slot: AvatarSlot) -> Option<Image> {
    match IMAGE_CACHE.with_borrow_mut(|cache| cache.lookup(path)) {
        Lookup::Hit(image) => return Some(image),
        Lookup::Failed => return None,
        Lookup::Miss => {}
    }
    let should_dispatch = AVATAR_WAITERS.with_borrow_mut(|waiters| {
        let is_new = !waiters.contains_key(path);
        let slots = waiters.entry(path.to_path_buf()).or_default();
        if !slots.contains(&slot) {
            slots.push(slot);
        }
        is_new
    });
    if should_dispatch {
        send_job(Lane::Avatar, path.to_path_buf());
    }
    None
}

fn cached_image(path: &Path) -> Option<Image> {
    match IMAGE_CACHE.with_borrow_mut(|cache| cache.lookup(path)) {
        Lookup::Hit(image) => Some(image),
        Lookup::Failed | Lookup::Miss => None,
    }
}

pub fn peek_thumbnail(path: &Path) -> Option<Image> {
    if is_animatable(path) {
        None
    } else {
        cached_image(path)
    }
}

pub fn peek_avatar(path: &Path) -> Option<Image> {
    cached_image(path)
}

pub fn record_media_need(unique_id: &str, thumbnail: Option<PathBuf>, avatar: Option<PathBuf>) {
    if thumbnail.is_none() && avatar.is_none() {
        MEDIA_NEEDS.with_borrow_mut(|needs| needs.remove(unique_id));
        return;
    }
    MEDIA_NEEDS.with_borrow_mut(|needs| {
        needs.insert(unique_id.to_owned(), MediaNeed { thumbnail, avatar });
    });
}

pub fn forget_all_media_needs() {
    MEDIA_NEEDS.with_borrow_mut(HashMap::clear);
}

pub fn record_avatar_need(slot: AvatarSlot, path: PathBuf) {
    AVATAR_NEEDS.with_borrow_mut(|needs| needs.insert(slot, path));
}

pub fn request_avatar(slot: &AvatarSlot) {
    let Some(path) = AVATAR_NEEDS.with_borrow(|needs| needs.get(slot).cloned()) else {
        return;
    };
    if let Some(image) = load_avatar_async(&path, slot.clone()) {
        notify_avatar_ready(slice::from_ref(slot), DecodeOutcome::Ready(&image));
    }
}

pub fn request_media(unique_id: &str) {
    let Some(need) = MEDIA_NEEDS.with_borrow(|needs| needs.get(unique_id).cloned()) else {
        return;
    };
    if let Some(thumbnail) = &need.thumbnail
        && let Some(img) = load_thumbnail(thumbnail, unique_id)
    {
        notify_ready(&[unique_id.to_owned()], DecodeOutcome::Ready(&img));
    }
    if let Some(avatar) = &need.avatar {
        let slot = AvatarSlot::Message(unique_id.to_owned());
        if let Some(img) = load_avatar_async(avatar, slot.clone()) {
            notify_avatar_ready(&[slot], DecodeOutcome::Ready(&img));
        }
    }
}

pub fn set_image_ready(ready: impl Fn(&str, DecodeOutcome<'_>) + 'static) {
    IMAGE_READY_FN.with_borrow_mut(|slot| *slot = Some(Rc::new(ready)));
}

pub fn set_avatar_ready(ready: impl Fn(&[AvatarSlot], DecodeOutcome<'_>) + 'static) {
    AVATAR_READY_FN.with_borrow_mut(|slot| *slot = Some(Rc::new(ready)));
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

struct RawFrame {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

struct RawAnimation {
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

fn decode_raw_animation(path: &Path) -> Option<RawAnimation> {
    let mut frames = Vec::new();
    let mut delays = Vec::new();
    let mut bytes: usize = 0;

    for frame in frames_of(path)? {
        if frames.len() >= ANIM_MAX_FRAMES {
            break;
        }
        let Ok(frame) = frame else { break };
        let delay = frame_delay(Duration::from(frame.delay()));
        let buffer = frame.into_buffer();
        let (source_width, source_height) = buffer.dimensions();

        if source_width > ANIM_MAX_DIMENSION || source_height > ANIM_MAX_DIMENSION {
            tracing::debug!(
                "animation at {} exceeds the {ANIM_MAX_DIMENSION}px dimension cap, showing a still",
                path.display()
            );
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

        bytes = bytes.saturating_add(width as usize * height as usize * 4);
        if bytes > ANIM_PER_ITEM_BUDGET {
            tracing::debug!(
                "animation at {} exceeds the per-item budget, showing a still",
                path.display()
            );
            return None;
        }

        frames.push(RawFrame {
            rgba: buffer.into_raw(),
            width,
            height,
        });
        delays.push(delay);
    }

    (frames.len() > 1).then_some(RawAnimation {
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

fn cached_animation(path: &Path) -> Option<Rc<Animation>> {
    ANIMATION_CACHE.with_borrow(|cache| cache.get(path).cloned().flatten())
}

fn on_animation_decoded(path: &Path, decoded: Option<RawAnimation>) {
    let remaining = ANIMATION_MEMORY_BUDGET.saturating_sub(cached_animation_bytes());
    let animation = decoded
        .filter(|raw| raw.bytes <= remaining)
        .map(|raw| Rc::new(raw.into_animation()));
    ANIMATION_CACHE.with_borrow_mut(|cache| {
        cache.insert(path.to_path_buf(), animation.clone());
    });

    let waiting = IN_FLIGHT
        .with_borrow_mut(|inflight| inflight.remove(path))
        .unwrap_or_default();

    let Some(animation) = animation else {
        for unique_id in &waiting {
            enqueue_decode(path, unique_id, Lane::Static);
        }
        return;
    };

    let now = Instant::now();
    PLAYBACKS.with_borrow_mut(|playbacks| {
        for unique_id in &waiting {
            if playbacks.contains_key(unique_id) {
                continue;
            }
            if playbacks.len() >= MAX_ACTIVE_ANIMATIONS {
                break;
            }
            playbacks.insert(
                unique_id.clone(),
                Playback {
                    path: path.to_path_buf(),
                    frame: 0,
                    next_at: now + animation.delay(0),
                    row_hint: 0,
                },
            );
        }
    });
    reschedule_animations();
    let first = animation
        .frame(0)
        .map_or(DecodeOutcome::Failed, DecodeOutcome::Ready);
    notify_ready(&waiting, first);
}

pub fn load_thumbnail(path: &Path, playback_key: &str) -> Option<Image> {
    if !is_animatable(path) {
        return request_thumbnail(path, playback_key);
    }
    let animation = match ANIMATION_CACHE.with_borrow(|cache| cache.get(path).cloned()) {
        Some(Some(animation)) => animation,
        Some(None) => return request_thumbnail(path, playback_key),
        None => {
            enqueue_decode(path, playback_key, Lane::Animation);
            return None;
        }
    };

    let (frame, is_new) = PLAYBACKS.with_borrow_mut(|playbacks| {
        if let Some(playback) = playbacks.get(playback_key) {
            return (playback.frame, false);
        }
        if playbacks.len() >= MAX_ACTIVE_ANIMATIONS {
            return (0, false);
        }
        playbacks.insert(
            playback_key.to_owned(),
            Playback {
                path: path.to_path_buf(),
                frame: 0,
                next_at: Instant::now() + animation.delay(0),
                row_hint: 0,
            },
        );
        (0, true)
    });

    if is_new {
        reschedule_animations();
    }

    animation.frame(frame).cloned()
}

fn request_thumbnail(path: &Path, unique_id: &str) -> Option<Image> {
    match IMAGE_CACHE.with_borrow_mut(|cache| cache.lookup(path)) {
        Lookup::Hit(image) => Some(image),
        Lookup::Failed => None,
        Lookup::Miss => {
            enqueue_decode(path, unique_id, Lane::Static);
            None
        }
    }
}

fn due_frames(now: Instant) -> Vec<DueFrame> {
    PLAYBACKS.with_borrow_mut(|playbacks| {
        let mut due = Vec::new();
        for (unique_id, playback) in playbacks.iter_mut() {
            if playback.next_at > now {
                continue;
            }
            let Some(animation) = cached_animation(&playback.path) else {
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

fn locate_row<T: Clone + 'static>(
    model: &VecModel<T>,
    entry_id: &dyn Fn(&T) -> String,
    unique_id: &str,
    hint: usize,
) -> Option<usize> {
    if let Some(entry) = model.row_data(hint)
        && entry_id(&entry) == unique_id
    {
        return Some(hint);
    }
    (0..model.row_count()).find(|&row| {
        model
            .row_data(row)
            .is_some_and(|e| entry_id(&e) == unique_id)
    })
}

fn forget_playbacks(gone: &[String]) {
    if gone.is_empty() {
        return;
    }
    let live_paths = PLAYBACKS.with_borrow_mut(|playbacks| {
        for unique_id in gone {
            playbacks.remove(unique_id);
        }
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

    let mut located = Vec::new();
    let mut gone = Vec::new();
    for item in due {
        let Some(row) = locate_row(timeline_model, entry_id, &item.unique_id, item.row_hint) else {
            gone.push(item.unique_id);
            continue;
        };
        if let Some(entry) = timeline_model.row_data(row) {
            let mut updated = entry;
            set_thumbnail(&mut updated, item.image);
            timeline_model.set_row_data(row, updated);
        }
        located.push((item.unique_id, row));
    }

    PLAYBACKS.with_borrow_mut(|playbacks| {
        for (unique_id, row) in located {
            if let Some(playback) = playbacks.get_mut(&unique_id) {
                playback.row_hint = row;
            }
        }
    });
    forget_playbacks(&gone);
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
