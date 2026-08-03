mod browser;
mod data;
mod dto;
mod login;
mod matrix;
mod media;
mod stickers;
mod storage;
mod timeline;
mod verification;

use std::sync::Arc;

use super::ui::SlintUiAdapter;
use crate::ports::browser::BrowserPort;
use crate::ports::matrix::AuthPort;
use crate::ports::media::MediaCache;
use crate::ports::storage::StoragePort;

const WINDOW_SIZE: (f32, f32) = (860.0, 1000.0);
const WINDOW_ENV: &str = "U2DM_DEMO_WINDOW";

pub fn matrix() -> Arc<dyn AuthPort> {
    Arc::new(matrix::DemoMatrix)
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

pub fn size_window_for_screenshots(ui: &SlintUiAdapter) {
    let (width, height) = requested_window_size().unwrap_or(WINDOW_SIZE);
    ui.set_window_size(width, height);
}

fn requested_window_size() -> Option<(f32, f32)> {
    let raw = std::env::var(WINDOW_ENV).ok()?;
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
