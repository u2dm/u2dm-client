mod animation;
mod cache;
mod requests;
mod waiters;
mod workers;

use std::sync::atomic::{AtomicU64, Ordering};

use animation::AnimationState;
pub use animation::{advance_animations, load_thumbnail, set_animation_tick};
use cache::ImageCache;
pub use cache::{Decoded, load_avatar_async, peek_avatar, peek_thumbnail};
use requests::{Needs, Request};
pub use requests::{
    forget_all_media_needs, record_avatar_need, record_media_need, record_sticker_need,
    request_avatar, request_media, request_sticker,
};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use waiters::Waiters;
pub use waiters::{AvatarSlot, DecodeOutcome, set_avatar_ready, set_image_ready};

use super::session::with_session;

const DISPLAY_MAX_DIMENSION: u32 = 512;

static LATEST_EPOCH: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
struct Epoch(u64);

impl Epoch {
    fn begin() -> Self {
        Self(LATEST_EPOCH.fetch_add(1, Ordering::Relaxed).wrapping_add(1))
    }

    fn is_current(self) -> bool {
        self.0 == LATEST_EPOCH.load(Ordering::Relaxed)
    }
}

pub struct MediaSession {
    epoch: Epoch,
    images: ImageCache,
    animations: AnimationState,
    needs: Needs,
    pending: Vec<Request>,
    waiters: Waiters,
}

impl Default for MediaSession {
    fn default() -> Self {
        Self {
            epoch: Epoch::begin(),
            images: ImageCache::default(),
            animations: AnimationState::default(),
            needs: Needs::default(),
            pending: Vec::new(),
            waiters: Waiters::default(),
        }
    }
}

fn with_media<R>(f: impl FnOnce(&mut MediaSession) -> R) -> R {
    with_session(|session| f(&mut session.media))
}

fn image_from_rgba(rgba: &[u8], width: u32, height: u32) -> Image {
    let pixels = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba, width, height);
    Image::from_rgba8(pixels)
}
