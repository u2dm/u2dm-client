mod animation;
mod cache;
mod requests;
mod waiters;
mod workers;

use std::cell::Cell;

pub use animation::{advance_animations, load_thumbnail, set_animation_tick};
pub use cache::{load_avatar_async, peek_avatar, peek_thumbnail};
pub use requests::{
    forget_all_media_needs, record_avatar_need, record_media_need, record_sticker_need,
    request_avatar, request_media, request_sticker,
};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
pub use waiters::{AvatarSlot, DecodeOutcome, set_avatar_ready, set_image_ready};

const DISPLAY_MAX_DIMENSION: u32 = 512;

thread_local! {
    static EPOCH: Cell<u64> = const { Cell::new(0) };
}

fn current_epoch() -> u64 {
    EPOCH.get()
}

fn is_stale(epoch: u64) -> bool {
    epoch != EPOCH.get()
}

fn discard_in_flight_decodes() {
    EPOCH.set(EPOCH.get().wrapping_add(1));
}

fn image_from_rgba(rgba: &[u8], width: u32, height: u32) -> Image {
    let pixels = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba, width, height);
    Image::from_rgba8(pixels)
}

pub fn clear_session_media() {
    discard_in_flight_decodes();
    requests::clear();
    waiters::clear();
    cache::clear();
    animation::clear();
    workers::clear();
}
