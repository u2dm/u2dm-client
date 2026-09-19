mod attachments;
pub mod audio;
mod browser;
pub mod catalog;
mod data;
mod dto;
mod files;
mod login;
mod matrix;
mod media;
pub mod probe;
mod reactions;
mod richtext;
mod stickers;
mod storage;
mod timeline;
mod verification;
mod videos;

use std::env;
use std::sync::Arc;

use super::ui::SlintUiAdapter;
use crate::ports::browser::BrowserPort;
use crate::ports::matrix::AuthPort;
use crate::ports::media::{MediaCache, MediaFilePort};
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
    Arc::new(storage::DemoStorage)
}

pub fn media_cache() -> Arc<dyn MediaCache> {
    Arc::new(media::DemoMediaCache)
}

pub fn browser() -> Arc<dyn BrowserPort> {
    Arc::new(browser::DemoBrowser)
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
