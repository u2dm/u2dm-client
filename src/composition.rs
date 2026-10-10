use std::sync::Arc;

use tokio::runtime::Handle;

#[cfg(feature = "matrix")]
use crate::adapters::browser::DesktopBrowser;
#[cfg(feature = "demo")]
use crate::adapters::demo;
#[cfg(feature = "matrix")]
use crate::adapters::matrix::MatrixAdapter;
#[cfg(feature = "matrix")]
use crate::adapters::media::DesktopMediaFiles;
#[cfg(feature = "matrix")]
use crate::adapters::storage::SecureStorage;
use crate::config::AppConfig;
use crate::ports::browser::BrowserPort;
use crate::ports::matrix::AuthPort;
use crate::ports::media::{MediaCache, MediaFilePort};
use crate::ports::storage::StoragePort;

#[cfg(not(any(feature = "matrix", feature = "demo")))]
compile_error!("enable the `matrix` or `demo` feature");

pub struct Backend {
    pub auth: Arc<dyn AuthPort>,
    pub storage: Arc<dyn StoragePort>,
    pub media_cache: Arc<dyn MediaCache>,
    pub media_files: Arc<dyn MediaFilePort>,
    pub browser: Arc<dyn BrowserPort>,
}

impl Backend {
    #[cfg(feature = "matrix")]
    pub fn select(cfg: &AppConfig, runtime: &Handle) -> Self {
        Self::demo().unwrap_or_else(|| Self::production(cfg, runtime))
    }

    #[cfg(not(feature = "matrix"))]
    pub fn select(_cfg: &AppConfig, _runtime: &Handle) -> Self {
        Self::fake()
    }

    #[cfg(all(feature = "matrix", feature = "demo"))]
    #[allow(clippy::unnecessary_wraps)]
    fn demo() -> Option<Self> {
        Some(Self::fake())
    }

    #[cfg(not(feature = "demo"))]
    fn demo() -> Option<Self> {
        None
    }

    #[cfg(feature = "demo")]
    fn fake() -> Self {
        tracing::info!("demo mode: serving fake rooms, spaces and timeline");
        demo::log_data_source();
        Self {
            auth: demo::matrix(),
            storage: demo::storage(),
            media_cache: demo::media_cache(),
            media_files: demo::media_files(),
            browser: demo::browser(),
        }
    }

    #[cfg(feature = "matrix")]
    fn production(cfg: &AppConfig, runtime: &Handle) -> Self {
        let adapter = MatrixAdapter::new(cfg.data_dir.clone(), cfg.cache_dir.clone());
        let media_cache = adapter.media_cache();
        Self {
            auth: Arc::new(adapter),
            storage: Arc::new(SecureStorage::new(&cfg.data_dir, runtime)),
            media_cache,
            media_files: Arc::new(DesktopMediaFiles::new()),
            browser: Arc::new(DesktopBrowser::new()),
        }
    }
}
