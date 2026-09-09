use std::sync::Arc;

use crate::adapters::browser::DesktopBrowser;
#[cfg(feature = "demo")]
use crate::adapters::demo;
use crate::adapters::matrix::MatrixAdapter;
use crate::adapters::media::DesktopMediaFiles;
use crate::adapters::storage::SecureStorage;
use crate::config::AppConfig;
use crate::ports::browser::BrowserPort;
use crate::ports::matrix::AuthPort;
use crate::ports::media::{MediaCache, MediaFilePort};
use crate::ports::storage::StoragePort;

pub struct Backend {
    pub auth: Arc<dyn AuthPort>,
    pub storage: Arc<dyn StoragePort>,
    pub media_cache: Arc<dyn MediaCache>,
    pub media_files: Arc<dyn MediaFilePort>,
    pub browser: Arc<dyn BrowserPort>,
}

impl Backend {
    pub fn select(cfg: &AppConfig) -> Self {
        Self::demo().unwrap_or_else(|| Self::production(cfg))
    }

    #[cfg(feature = "demo")]
    #[allow(clippy::unnecessary_wraps)]
    fn demo() -> Option<Self> {
        tracing::info!("demo mode: serving fake rooms, spaces and timeline");
        demo::log_data_source();
        Some(Self {
            auth: demo::matrix(),
            storage: demo::storage(),
            media_cache: demo::media_cache(),
            media_files: demo::media_files(),
            browser: demo::browser(),
        })
    }

    #[cfg(not(feature = "demo"))]
    fn demo() -> Option<Self> {
        None
    }

    fn production(cfg: &AppConfig) -> Self {
        let adapter = MatrixAdapter::new(cfg.data_dir.clone(), cfg.cache_dir.clone());
        let media_cache = adapter.media_cache();
        Self {
            auth: Arc::new(adapter),
            storage: Arc::new(SecureStorage::new(&cfg.data_dir)),
            media_cache,
            media_files: Arc::new(DesktopMediaFiles::new()),
            browser: Arc::new(DesktopBrowser::new()),
        }
    }
}
