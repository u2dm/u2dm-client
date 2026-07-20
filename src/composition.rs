use std::sync::Arc;

#[cfg(feature = "demo")]
use crate::adapters::demo;
use crate::adapters::matrix::MatrixAdapter;
use crate::adapters::storage::SecureStorage;
use crate::config::AppConfig;
use crate::ports::matrix::AuthPort;
use crate::ports::media::MediaCache;
use crate::ports::storage::StoragePort;

pub struct Backend {
    pub auth: Arc<dyn AuthPort>,
    pub storage: Arc<dyn StoragePort>,
    pub media_cache: Arc<dyn MediaCache>,
}

impl Backend {
    pub fn select(cfg: &AppConfig) -> Self {
        Self::demo().unwrap_or_else(|| Self::production(cfg))
    }

    #[cfg(feature = "demo")]
    #[allow(clippy::unnecessary_wraps)]
    fn demo() -> Option<Self> {
        tracing::info!("demo mode: serving fake rooms, spaces and timeline");
        Some(Self {
            auth: demo::matrix(),
            storage: demo::storage(),
            media_cache: demo::media_cache(),
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
        }
    }
}
