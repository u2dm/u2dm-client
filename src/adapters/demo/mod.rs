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
    ui.set_window_size(WINDOW_SIZE.0, WINDOW_SIZE.1);
}
