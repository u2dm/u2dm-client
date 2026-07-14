mod data;
mod dto;
mod matrix;
mod media;
mod storage;

use std::sync::Arc;

use super::ui::SlintUiAdapter;
use crate::ports::matrix::MatrixPort;
use crate::ports::media::MediaCache;
use crate::ports::storage::StoragePort;

const WINDOW_SIZE: (f32, f32) = (860.0, 1000.0);

pub fn matrix() -> Arc<dyn MatrixPort> {
    Arc::new(matrix::DemoMatrix::default())
}

pub fn storage() -> Arc<dyn StoragePort> {
    Arc::new(storage::DemoStorage::default())
}

pub fn media_cache() -> Arc<dyn MediaCache> {
    Arc::new(media::DemoMediaCache)
}

pub fn size_window_for_screenshots(ui: &SlintUiAdapter) {
    ui.set_window_size(WINDOW_SIZE.0, WINDOW_SIZE.1);
}
