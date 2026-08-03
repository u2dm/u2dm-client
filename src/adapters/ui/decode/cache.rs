use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use slint::Image;

use super::waiters::{AvatarSlot, DecodeOutcome};
use super::workers::Lane;
use super::{DISPLAY_MAX_DIMENSION, animation, image_from_rgba, waiters};

const IMAGE_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;

const DECODE_MAX_DIMENSION: u32 = 4096;
const DECODE_MAX_ALLOC: u64 = 4 * DECODE_MAX_DIMENSION as u64 * DECODE_MAX_DIMENSION as u64;

thread_local! {
    static IMAGES: RefCell<ImageCache> = RefCell::new(ImageCache::default());
}

pub enum Decoded {
    Ready(Image),
    Failed,
    Pending,
}

struct CachedImage {
    image: Option<Image>,
    bytes: usize,
    tick: u64,
}

#[derive(Default)]
struct ImageCache {
    entries: HashMap<PathBuf, CachedImage>,
    total_bytes: usize,
    tick: u64,
}

impl ImageCache {
    fn lookup(&mut self, path: &Path) -> Decoded {
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        let Some(entry) = self.entries.get_mut(path) else {
            return Decoded::Pending;
        };
        entry.tick = tick;
        match &entry.image {
            Some(image) => Decoded::Ready(image.clone()),
            None => Decoded::Failed,
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

fn decode_limits() -> image::Limits {
    let mut limits = image::Limits::no_limits();
    limits.max_image_width = Some(DECODE_MAX_DIMENSION);
    limits.max_image_height = Some(DECODE_MAX_DIMENSION);
    limits.max_alloc = Some(DECODE_MAX_ALLOC);
    limits
}

pub(super) fn decode_rgba(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
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

pub(super) fn on_decoded(path: &Path, decoded: Option<(Vec<u8>, u32, u32)>, epoch: u64) {
    if super::is_stale(epoch) {
        return;
    }
    let decoded = decoded.map(|(bytes, width, height)| {
        let len = bytes.len();
        (image_from_rgba(&bytes, width, height), len)
    });
    let bytes = decoded.as_ref().map_or(0, |(_, len)| *len);
    let image = decoded.map(|(image, _)| image);
    IMAGES.with_borrow_mut(|images| images.insert(path.to_path_buf(), image.clone(), bytes));

    let outcome = image
        .as_ref()
        .map_or(DecodeOutcome::Failed, DecodeOutcome::Ready);
    waiters::deliver(path, outcome);
}

fn cached(path: &Path) -> Decoded {
    IMAGES.with_borrow_mut(|images| images.lookup(path))
}

pub fn peek_thumbnail(path: &Path) -> Decoded {
    if animation::is_animatable(path) {
        Decoded::Pending
    } else {
        cached(path)
    }
}

pub fn peek_avatar(path: &Path) -> Option<Image> {
    match cached(path) {
        Decoded::Ready(image) => Some(image),
        Decoded::Failed | Decoded::Pending => None,
    }
}

pub fn load_avatar_async(path: &Path, slot: AvatarSlot) -> Option<Image> {
    match cached(path) {
        Decoded::Ready(image) => Some(image),
        Decoded::Failed => None,
        Decoded::Pending => {
            waiters::enqueue_avatar(path, slot);
            None
        }
    }
}

pub(super) fn request_thumbnail(path: &Path, unique_id: &str) -> Decoded {
    match cached(path) {
        Decoded::Pending => {
            waiters::enqueue_media(path, unique_id, Lane::Static);
            Decoded::Pending
        }
        decoded => decoded,
    }
}

pub(super) fn clear() {
    IMAGES.with_borrow_mut(|images| *images = ImageCache::default());
}
