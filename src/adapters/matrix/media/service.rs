use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use matrix_sdk::Client;
use matrix_sdk::media::{MediaFormat, MediaRequestParameters};
use matrix_sdk::ruma::OwnedMxcUri;
use matrix_sdk::ruma::events::room::MediaSource;
use tokio::fs;
use tokio::sync::Semaphore;
use tokio::time::{sleep, timeout};

use super::cache::{DiskCache, FailureTracker};
use super::{avatar_key, thumb_key, thumbnail_format};
use crate::domain::models::{MessageBody, ThumbnailOutcome, TimelineMessage};
use crate::error::{AppError, Result};
use crate::util::hex_encode_id;

const MAX_CONCURRENT_DOWNLOADS: usize = 6;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);
const FULL_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const RETRY_MAX_ATTEMPTS: u32 = 3;
const RETRY_BACKOFF_BASE: Duration = Duration::from_millis(500);
const MAX_MEDIA_BYTES: usize = 20 * 1024 * 1024;
const MAX_FULL_MEDIA_BYTES: usize = 100 * 1024 * 1024;

pub(crate) struct MediaService {
    media_dir: PathBuf,
    semaphore: Semaphore,
    cache: StdMutex<DiskCache>,
    failures: StdMutex<FailureTracker>,
}

impl MediaService {
    pub(crate) fn new(cache_dir: &Path) -> Arc<Self> {
        let media_dir = cache_dir.join("media-cache");
        let cache = DiskCache::load(&media_dir);
        Arc::new(Self {
            media_dir,
            semaphore: Semaphore::new(MAX_CONCURRENT_DOWNLOADS),
            cache: StdMutex::new(cache),
            failures: StdMutex::new(FailureTracker::default()),
        })
    }

    pub(crate) fn media_dir(&self) -> &Path {
        &self.media_dir
    }

    pub(crate) fn cache_get(&self, key: &str) -> Option<PathBuf> {
        self.cache.lock().ok()?.get(key)
    }

    pub(crate) fn is_failed(&self, key: &str) -> bool {
        self.failures.lock().is_ok_and(|f| f.should_skip(key))
    }

    fn record_failure(&self, key: &str) {
        if let Ok(mut failures) = self.failures.lock() {
            failures.record_failure(key);
        }
    }

    fn record_success(&self, key: &str) {
        if let Ok(mut failures) = self.failures.lock() {
            failures.record_success(key);
        }
    }

    fn store(&self, key: &str, path: PathBuf, bytes: u64) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(key, path, bytes);
        }
    }

    pub(crate) async fn clear(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
        if let Ok(mut failures) = self.failures.lock() {
            failures.clear();
        }
        if let Err(e) = fs::remove_dir_all(&self.media_dir).await
            && e.kind() != ErrorKind::NotFound
        {
            tracing::warn!("failed to clear media cache dir: {e}");
        }
    }

    async fn download(
        &self,
        client: &Client,
        request: &MediaRequestParameters,
        download_timeout: Duration,
        max_bytes: usize,
    ) -> Option<Vec<u8>> {
        let _permit = self.semaphore.acquire().await.ok()?;

        let mut backoff = RETRY_BACKOFF_BASE;
        for attempt in 1..=RETRY_MAX_ATTEMPTS {
            if let Some(data) = attempt_download(client, request, attempt, download_timeout).await {
                if data.len() > max_bytes {
                    tracing::debug!(
                        "media payload {} bytes exceeds the {max_bytes} byte limit",
                        data.len()
                    );
                    return None;
                }
                return Some(data);
            }
            if attempt < RETRY_MAX_ATTEMPTS {
                sleep(backoff).await;
                backoff = backoff.saturating_mul(2);
            }
        }
        None
    }

    pub(crate) async fn fetch_and_materialize(
        &self,
        client: &Client,
        source: MediaSource,
        cache_key: &str,
        cache_stem: &Path,
        format: MediaFormat,
    ) -> Option<PathBuf> {
        if self.is_failed(cache_key) {
            return None;
        }

        let request = MediaRequestParameters { source, format };
        let Some(data) = self
            .download(client, &request, DOWNLOAD_TIMEOUT, MAX_MEDIA_BYTES)
            .await
        else {
            self.record_failure(cache_key);
            return None;
        };

        let cache_path = cache_stem.with_extension(ext_from_magic(&data));
        if let Err(e) = fs::write(&cache_path, &data).await {
            tracing::warn!("failed to write materialized media: {e}");
            self.record_failure(cache_key);
            return None;
        }

        self.store(cache_key, cache_path.clone(), data.len() as u64);
        self.record_success(cache_key);
        Some(cache_path)
    }

    pub(crate) async fn enrich_thumbnail(
        &self,
        client: &Client,
        media_sources: &StdMutex<HashMap<String, MediaSource>>,
        msg: &TimelineMessage,
    ) -> ThumbnailOutcome {
        let MessageBody::Image { meta, .. } = &msg.body else {
            return ThumbnailOutcome::Unchanged;
        };
        let Some(event_id) = msg.event_id.as_ref() else {
            return ThumbnailOutcome::Unchanged;
        };
        let event_id = &event_id.0;
        let cache_key = thumb_key(event_id);

        if self.cache_get(&cache_key).is_some() {
            return ThumbnailOutcome::Unchanged;
        }

        let animated = super::is_animated_mime(meta.mimetype.as_deref());
        let source = if animated {
            super::lookup_full_media_source(media_sources, event_id)
        } else {
            super::lookup_media_source(media_sources, event_id)
        };

        let materialized_path = if let Some(source) = source {
            let format = if animated {
                MediaFormat::File
            } else {
                thumbnail_format()
            };
            let cache_stem = self.media_dir.join(hex_encode_id(event_id));
            self.fetch_and_materialize(client, source, &cache_key, &cache_stem, format)
                .await
        } else {
            None
        };

        if materialized_path.is_some() {
            ThumbnailOutcome::Ready
        } else {
            ThumbnailOutcome::Failed
        }
    }

    pub(crate) async fn enrich_avatar(&self, client: &Client, msg: &TimelineMessage) -> bool {
        let Some(mxc_url) = &msg.sender_avatar_url else {
            return false;
        };
        let cache_key = avatar_key(&msg.sender);

        if self.cache_get(&cache_key).is_some() {
            return false;
        }

        let cache_stem = self.avatars_dir().join(hex_encode_id(&msg.sender));
        let source = MediaSource::Plain(mxc_url.as_str().into());
        self.fetch_and_materialize(client, source, &cache_key, &cache_stem, thumbnail_format())
            .await
            .is_some()
    }

    pub(crate) async fn fetch_avatar_by_mxc(
        &self,
        client: &Client,
        cache_key: &str,
        mxc: OwnedMxcUri,
    ) -> Option<PathBuf> {
        if self.cache_get(cache_key).is_some() {
            return None;
        }
        if let Err(e) = fs::create_dir_all(self.avatars_dir()).await {
            tracing::warn!("failed to create avatar dir: {e}");
            return None;
        }
        let cache_stem = self.avatars_dir().join(hex_encode_id(mxc.as_str()));
        let source = MediaSource::Plain(mxc);
        self.fetch_and_materialize(client, source, cache_key, &cache_stem, thumbnail_format())
            .await
    }

    pub(crate) async fn fetch_user_avatar(&self, client: &Client) -> Option<PathBuf> {
        let cached = client.account().get_cached_avatar_url().await;
        let mxc = match cached {
            Ok(Some(mxc)) => mxc,
            _ => match client.account().get_avatar_url().await {
                Ok(Some(mxc)) => mxc,
                Ok(None) => return None,
                Err(e) => {
                    tracing::debug!("failed to fetch user avatar url: {e}");
                    return None;
                }
            },
        };

        let key = format!("user-avatar:{mxc}");
        if let Some(path) = self.cache_get(&key) {
            return Some(path);
        }
        self.fetch_avatar_by_mxc(client, &key, mxc).await
    }

    pub(crate) async fn download_media(
        &self,
        client: &Client,
        media_sources: &StdMutex<HashMap<String, MediaSource>>,
        event_id: &str,
        thumbnail: bool,
    ) -> Result<Vec<u8>> {
        let key = if thumbnail {
            format!("{event_id}:thumb")
        } else {
            event_id.to_string()
        };

        let source = media_sources
            .lock()
            .map_err(|e| AppError::Other(format!("media source lock poisoned: {e}")))?
            .get(&key)
            .cloned()
            .or_else(|| {
                if thumbnail {
                    media_sources.lock().ok()?.get(event_id).cloned()
                } else {
                    None
                }
            })
            .ok_or_else(|| AppError::Other(format!("no media source for event {event_id}")))?;

        let (format, download_timeout, max_bytes) = if thumbnail {
            (thumbnail_format(), DOWNLOAD_TIMEOUT, MAX_MEDIA_BYTES)
        } else {
            (
                MediaFormat::File,
                FULL_DOWNLOAD_TIMEOUT,
                MAX_FULL_MEDIA_BYTES,
            )
        };

        let request = MediaRequestParameters { source, format };
        self.download(client, &request, download_timeout, max_bytes)
            .await
            .ok_or_else(|| {
                AppError::Other(format!(
                    "media download failed or exceeded the {max_bytes} byte limit for event {event_id}"
                ))
            })
    }

    pub(crate) fn needs_media_download(&self, msg: &TimelineMessage) -> bool {
        let needs_thumbnail = matches!(&msg.body, MessageBody::Image { .. })
            && msg.event_id.as_ref().is_some_and(|event_id| {
                let key = thumb_key(&event_id.0);
                self.cache_get(&key).is_none() && !self.is_failed(&key)
            });
        let needs_avatar = msg.sender_avatar_url.is_some() && {
            let key = avatar_key(&msg.sender);
            self.cache_get(&key).is_none() && !self.is_failed(&key)
        };
        needs_thumbnail || needs_avatar
    }

    fn avatars_dir(&self) -> PathBuf {
        self.media_dir.join("avatars")
    }
}

async fn attempt_download(
    client: &Client,
    request: &MediaRequestParameters,
    attempt: u32,
    download_timeout: Duration,
) -> Option<Vec<u8>> {
    match timeout(
        download_timeout,
        client.media().get_media_content(request, true),
    )
    .await
    {
        Ok(Ok(data)) => Some(data),
        Ok(Err(e)) => {
            tracing::debug!("media download attempt {attempt} failed: {e}");
            None
        }
        Err(_) => {
            tracing::debug!("media download attempt {attempt} timed out");
            None
        }
    }
}

fn ext_from_magic(data: &[u8]) -> &'static str {
    infer::get(data).map_or("png", |t| t.extension())
}
