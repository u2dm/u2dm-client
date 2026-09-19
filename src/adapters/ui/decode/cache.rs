use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageError, ImageReader, ImageResult};
use slint::Image;

use super::requests::PreviewPick;
use super::slots::{AvatarSlot, MediaSlot};
use super::waiters::DecodeOutcome;
use super::workers::Lane;
use super::{DISPLAY_MAX_DIMENSION, Epoch, animation, image_from_rgba, waiters, with_media};

const IMAGE_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;

const DECODE_MAX_PIXELS: u64 = 8192 * 8192;
const DECODE_MAX_ALLOC: u64 = 4 * DECODE_MAX_PIXELS;

type Rgba = (Vec<u8>, u32, u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeFailure {
    UnsupportedFormat,
    OverBudget,
    Damaged,
    Unreadable,
}

impl DecodeFailure {
    fn of(error: &ImageError) -> Self {
        match error {
            ImageError::Unsupported(_) => Self::UnsupportedFormat,
            ImageError::Limits(_) => Self::OverBudget,
            ImageError::Decoding(_) => Self::Damaged,
            ImageError::IoError(io) if io.kind() == ErrorKind::UnexpectedEof => Self::Damaged,
            ImageError::IoError(_) | ImageError::Encoding(_) | ImageError::Parameter(_) => {
                Self::Unreadable
            }
        }
    }
}

pub enum Decoded {
    Ready(Image),
    Failed(DecodeFailure),
    Pending,
}

struct CachedImage {
    image: Result<Image, DecodeFailure>,
    bytes: usize,
    tick: u64,
}

#[derive(Default)]
pub(super) struct ImageCache {
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
            Ok(image) => Decoded::Ready(image.clone()),
            Err(failure) => Decoded::Failed(*failure),
        }
    }

    fn insert(&mut self, path: PathBuf, image: Result<Image, DecodeFailure>, bytes: usize) {
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

    fn forget(&mut self, path: &Path) {
        if let Some(entry) = self.entries.remove(path) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
        }
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
    limits.max_alloc = Some(DECODE_MAX_ALLOC);
    limits
}

fn read_image(path: &Path) -> ImageResult<DynamicImage> {
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    reader.limits(decode_limits());
    reader.decode()
}

pub(super) fn decode_rgba(path: &Path) -> Result<Rgba, DecodeFailure> {
    let decoded = read_image(path).map_err(|e| {
        tracing::debug!("could not decode {}: {e}", path.display());
        DecodeFailure::of(&e)
    })?;

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
        .and_then(|pixels| pixels.checked_mul(4));
    let raw = rgba.into_raw();
    (Some(raw.len()) == expected_len)
        .then_some((raw, width, height))
        .ok_or(DecodeFailure::Unreadable)
}

pub(super) fn on_decoded(path: &Path, decoded: Result<Rgba, DecodeFailure>, epoch: Epoch) {
    if !epoch.is_current() {
        return;
    }
    let decoded = decoded.map(|(bytes, width, height)| {
        let len = bytes.len();
        (image_from_rgba(&bytes, width, height), len)
    });
    let bytes = decoded.as_ref().map_or(0, |(_, len)| *len);
    let image = decoded.map(|(image, _)| image);
    with_media(|media| {
        media
            .images
            .insert(path.to_path_buf(), image.clone(), bytes);
    });

    let outcome = match &image {
        Ok(image) => DecodeOutcome::Ready(image),
        Err(failure) => DecodeOutcome::Failed(*failure),
    };
    waiters::deliver(path, outcome);
}

fn cached(path: &Path) -> Decoded {
    with_media(|media| media.images.lookup(path))
}

pub fn peek_thumbnail(path: &Path, slot: &MediaSlot) -> Decoded {
    match animation::playing_frame(path, slot) {
        Some(frame) => Decoded::Ready(frame),
        None => cached(path),
    }
}

pub fn peek_avatar(path: &Path) -> Option<Image> {
    match cached(path) {
        Decoded::Ready(image) => Some(image),
        Decoded::Failed(_) | Decoded::Pending => None,
    }
}

pub fn load_avatar_async(path: Option<&Path>, slot: AvatarSlot) -> Option<Image> {
    with_media(|media| media.needs.expect_avatar(&slot, path));
    let path = path?;
    match cached(path) {
        Decoded::Ready(image) => Some(image),
        Decoded::Failed(_) => None,
        Decoded::Pending => {
            waiters::enqueue_avatar(path, slot);
            None
        }
    }
}

pub fn load_attachment_preview(pick: u64, path: Option<&Path>) -> Option<Image> {
    let slot = AvatarSlot::AttachmentPreview { pick };
    with_media(|media| {
        if let (PreviewPick::New, Some(path)) = (media.needs.adopt_preview(&slot), path) {
            media.images.forget(path);
        }
    });
    load_avatar_async(path, slot)
}

pub(super) fn request_thumbnail(path: &Path, slot: &MediaSlot) -> Decoded {
    match cached(path) {
        Decoded::Pending => {
            waiters::enqueue_media(path, slot, Lane::Static);
            Decoded::Pending
        }
        decoded => decoded,
    }
}
