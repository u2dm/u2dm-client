mod attachments;
pub mod audio;
mod browser;
mod call_media;
mod calls;
pub mod catalog;
mod create_room;
mod data;
mod dto;
mod encryption;
mod files;
mod handoff;
mod identity;
mod invite_people;
mod invites;
mod links;
mod login;
mod logout;
mod matrix;
mod media;
mod message_menu;
mod msgtypes;
mod names;
pub mod notifications;
mod pinned;
mod polls;
mod powers;
mod presence;
pub mod probe;
mod reactions;
mod receipts;
mod richtext;
mod room_info;
mod shields;
mod source;
mod space_index;
mod stickers;
mod storage;
mod sync;
mod timeline;
mod typing;
mod upgrades;
mod user_info;
mod verification;
mod videos;

use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use super::ui::SlintUiAdapter;
use crate::ports::browser::BrowserPort;
use crate::ports::call::{CallMediaPort, CallVideoPort};
use crate::ports::handoff::HandoffPort;
use crate::ports::matrix::AuthPort;
use crate::ports::media::{MediaCache, MediaFilePort};
use crate::ports::notifier::NotifierPort;
use crate::ports::storage::StoragePort;

const WINDOW_SIZE: (f32, f32) = (860.0, 1000.0);
const WINDOW_ENV: &str = "U2DM_DEMO_WINDOW";

pub fn matrix() -> Arc<dyn AuthPort> {
    Arc::new(matrix::DemoMatrix)
}

pub fn log_data_source() {
    let (rooms, spaces, timelines) = data::counts();
    tracing::info!(
        path = %data::source_path(),
        rooms,
        spaces,
        timelines,
        "demo mode: loaded the fixture"
    );
    if let Some(error) = data::load_error() {
        tracing::error!(
            error,
            "demo mode: the fixture did not load, so the app starts empty"
        );
    }
}

pub fn storage() -> Arc<dyn StoragePort> {
    Arc::new(storage::DemoStorage::from_env())
}

pub fn media_cache() -> Arc<dyn MediaCache> {
    Arc::new(media::DemoMediaCache)
}

pub fn browser() -> Arc<dyn BrowserPort> {
    Arc::new(browser::DemoBrowser)
}

pub fn notifier() -> Arc<dyn NotifierPort> {
    notifications::notifier()
}

pub fn call_media() -> (Arc<dyn CallMediaPort>, Arc<dyn CallVideoPort>) {
    let demo = Arc::new(call_media::DemoCallMedia::default());
    let media: Arc<dyn CallMediaPort> = Arc::<call_media::DemoCallMedia>::clone(&demo);
    (media, demo)
}

pub fn handoff() -> Arc<dyn HandoffPort> {
    handoff::port()
}

pub fn handoff_socket() -> Option<PathBuf> {
    handoff::socket()
}

pub fn media_files() -> Arc<dyn MediaFilePort> {
    Arc::new(files::DemoMediaFiles::new())
}

pub fn configure_audio() {
    if audio::scenario().silent {
        SlintUiAdapter::prefer_silent_audio();
    }
}

pub fn size_window_for_screenshots(ui: &SlintUiAdapter) {
    let (width, height) = requested_window_size().unwrap_or(WINDOW_SIZE);
    ui.set_window_size(width, height);
}

fn requested_window_size() -> Option<(f32, f32)> {
    let raw = env::var(WINDOW_ENV).ok()?;
    let (width, height) = raw.split_once(['x', 'X'])?;
    let parsed = (width.trim().parse().ok()?, height.trim().parse().ok()?);
    tracing::info!(
        target: "u2dm::adapters::demo",
        width = parsed.0,
        height = parsed.1,
        "demo mode: overriding the window size"
    );
    Some(parsed)
}
