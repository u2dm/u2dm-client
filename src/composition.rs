use std::path::PathBuf;
use std::sync::Arc;

use tokio::runtime::Handle;

#[cfg(feature = "matrix")]
use crate::adapters::browser::DesktopBrowser;
#[cfg(feature = "matrix")]
use crate::adapters::call_media;
#[cfg(feature = "demo")]
use crate::adapters::demo;
#[cfg(feature = "matrix")]
use crate::adapters::handoff::{DesktopHandoff, SilentHandoff, socket_in};
#[cfg(feature = "matrix")]
use crate::adapters::matrix::MatrixAdapter;
#[cfg(feature = "matrix")]
use crate::adapters::media::DesktopMediaFiles;
#[cfg(feature = "matrix")]
use crate::adapters::notifier;
#[cfg(feature = "matrix")]
use crate::adapters::storage::SecureStorage;
use crate::config::AppConfig;
use crate::ports::browser::BrowserPort;
use crate::ports::call::CallMediaPort;
use crate::ports::handoff::HandoffPort;
use crate::ports::matrix::AuthPort;
#[cfg(feature = "matrix")]
use crate::ports::matrix::LocalDataOwnership;
use crate::ports::media::{MediaCache, MediaFilePort};
use crate::ports::notifier::NotifierPort;
use crate::ports::storage::StoragePort;

#[cfg(not(any(feature = "matrix", feature = "demo")))]
compile_error!("enable the `matrix` or `demo` feature");

pub struct Backend {
    pub auth: Arc<dyn AuthPort>,
    pub storage: Arc<dyn StoragePort>,
    pub media_cache: Arc<dyn MediaCache>,
    pub media_files: Arc<dyn MediaFilePort>,
    pub browser: Arc<dyn BrowserPort>,
    pub notifier: Arc<dyn NotifierPort>,
    pub handoff: Arc<dyn HandoffPort>,
    pub call_media: Option<Arc<dyn CallMediaPort>>,
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
    pub fn handoff_socket(_cfg: &AppConfig) -> Option<PathBuf> {
        demo::handoff_socket()
    }

    #[cfg(not(feature = "demo"))]
    #[allow(clippy::unnecessary_wraps)]
    pub fn handoff_socket(cfg: &AppConfig) -> Option<PathBuf> {
        Some(socket_in(&cfg.data_dir))
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
            notifier: demo::notifier(),
            handoff: demo::handoff(),
            call_media: Some(demo::call_media()),
        }
    }

    #[cfg(feature = "matrix")]
    fn production(cfg: &AppConfig, runtime: &Handle) -> Self {
        let adapter = MatrixAdapter::new(cfg.data_dir.clone(), cfg.cache_dir.clone());
        let media_cache = adapter.media_cache();
        let handoff: Arc<dyn HandoffPort> = match adapter.local_data_ownership() {
            LocalDataOwnership::Exclusive => {
                Arc::new(DesktopHandoff::new(socket_in(&cfg.data_dir)))
            }
            LocalDataOwnership::AnotherInstance { .. }
            | LocalDataOwnership::Undetermined { .. } => Arc::new(SilentHandoff),
        };
        Self {
            auth: Arc::new(adapter),
            storage: Arc::new(SecureStorage::new(&cfg.data_dir, runtime)),
            media_cache,
            media_files: Arc::new(DesktopMediaFiles::new()),
            browser: Arc::new(DesktopBrowser::new()),
            notifier: notifier::desktop(),
            handoff,
            call_media: call_media::production(),
        }
    }
}
