use std::path::{Path, PathBuf};
use std::time::Duration;

use matrix_sdk::media::MediaRequestParameters;
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::{Client, Room};
use tokio::time::{sleep, timeout};

use super::{DownloadLimits, Fetch, Materialized, MediaService, Playable};
use crate::adapters::matrix::media::{MediaLane, bounded, file_content, source};
use crate::adapters::private_fs;
use crate::domain::media::{MediaFailure, MediaRendition, MediaResult};
use crate::error::{AppError, Result};

pub(super) const MAX_CONCURRENT_DOWNLOADS: usize = 6;

pub(super) const MAX_CONCURRENT_FULL_DOWNLOADS: usize = 2;

pub(super) const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) const FULL_DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(2);

const RETRY_MAX_ATTEMPTS: u32 = 3;

const RETRY_BACKOFF_BASE: Duration = Duration::from_millis(500);

impl MediaService {
    async fn download(
        &self,
        client: &Client,
        request: &MediaRequestParameters,
        limits: DownloadLimits,
    ) -> MediaResult<Vec<u8>> {
        let DownloadLimits {
            timeout: download_timeout,
            max_bytes,
            full,
        } = limits;
        let semaphore = if full {
            &self.full_semaphore
        } else {
            &self.semaphore
        };
        let mut backoff = RETRY_BACKOFF_BASE;
        for attempt in 1..=RETRY_MAX_ATTEMPTS {
            let fetched = {
                let _permit = semaphore
                    .acquire()
                    .await
                    .map_err(|_| MediaFailure::Download)?;
                attempt_download(client, request, attempt, download_timeout, max_bytes).await
            };
            match fetched {
                Err(MediaFailure::Download) if attempt < RETRY_MAX_ATTEMPTS => {
                    sleep(backoff).await;
                    backoff = backoff.saturating_mul(2);
                }
                settled => return settled,
            }
        }
        Err(MediaFailure::Download)
    }

    pub(crate) async fn materialize_playable(
        &self,
        room: &Room,
        event_id: &str,
        playable: Playable,
    ) -> MediaResult<Materialized> {
        let source = source::resolve(room, event_id, MediaLane::FullFile).await?;
        let content = file_content(&source);
        let cache_key = playable.key(&content);
        if let Some(path) = self.cache_get(&cache_key) {
            return Ok(Materialized { content, path });
        }
        if let Some(reason) = self.failure(&cache_key) {
            return Err(reason);
        }
        let dir = self.playable_dir(playable).ok_or(MediaFailure::Storage)?;
        if let Err(e) = private_fs::create_dir(&dir).await {
            tracing::warn!("failed to create the {} dir: {e}", playable.noun());
            return Err(MediaFailure::Storage);
        }
        let cache_stem = dir.join(content.as_str());
        let path = self
            .fetch_and_materialize(
                &room.client(),
                source,
                &cache_key,
                &cache_stem,
                Fetch::launchable(playable),
            )
            .await?;
        Ok(Materialized { content, path })
    }

    pub(crate) async fn fetch_and_materialize(
        &self,
        client: &Client,
        source: MediaSource,
        cache_key: &str,
        cache_stem: &Path,
        fetch: Fetch,
    ) -> MediaResult<PathBuf> {
        self.flights
            .join(cache_key)
            .outcome(|| self.materialize(client, source, cache_key, cache_stem, fetch))
            .await
    }

    async fn materialize(
        &self,
        client: &Client,
        source: MediaSource,
        cache_key: &str,
        cache_stem: &Path,
        fetch: Fetch,
    ) -> MediaResult<PathBuf> {
        let Fetch {
            format,
            limits,
            naming,
        } = fetch;
        if let Some(cached) = self.cache_get(cache_key) {
            return Ok(cached);
        }
        if let Some(reason) = self.failure(cache_key) {
            return Err(reason);
        }

        let request = MediaRequestParameters { source, format };
        let data = match self.download(client, &request, limits).await {
            Ok(data) => data,
            Err(reason) => {
                self.record_failure(cache_key, reason);
                return Err(reason);
            }
        };

        let extension = match naming.extension(&data) {
            Ok(extension) => extension,
            Err(reason) => {
                self.record_failure(cache_key, reason);
                return Err(reason);
            }
        };
        let cache_path = cache_stem.with_extension(extension);
        if let Err(e) = private_fs::write_atomically(&cache_path, &data).await {
            tracing::warn!("failed to write materialized media: {e}");
            self.record_failure(cache_key, MediaFailure::Storage);
            return Err(MediaFailure::Storage);
        }

        self.store(cache_key, cache_path.clone(), data.len() as u64)
            .await;
        self.record_success(cache_key);
        Ok(cache_path)
    }

    pub(crate) async fn download_media(
        &self,
        room: &Room,
        event_id: &str,
        rendition: MediaRendition,
    ) -> Result<Vec<u8>> {
        let (lane, limits) = match rendition {
            MediaRendition::Thumbnail => (MediaLane::Thumbnail, DownloadLimits::thumbnail()),
            MediaRendition::FullFile => (MediaLane::FullFile, DownloadLimits::full_file()),
        };
        let source = source::resolve(room, event_id, lane)
            .await
            .map_err(|reason| {
                AppError::Other(format!("no media source for event {event_id} ({reason:?})"))
            })?;
        let max_bytes = limits.max_bytes;

        let request = MediaRequestParameters {
            source,
            format: lane.format(),
        };
        self.download(&room.client(), &request, limits)
            .await
            .map_err(|reason| {
                AppError::Other(format!(
                    "media download for event {event_id} failed: {reason:?} \
                     (limit {max_bytes} bytes)"
                ))
            })
    }
}

async fn attempt_download(
    client: &Client,
    request: &MediaRequestParameters,
    attempt: u32,
    download_timeout: Duration,
    max_bytes: usize,
) -> MediaResult<Vec<u8>> {
    let fetched = timeout(
        download_timeout,
        bounded::fetch(client, request, max_bytes, download_timeout),
    )
    .await;
    match fetched {
        Ok(Ok(data)) => Ok(data),
        Ok(Err(reason)) => {
            tracing::debug!("media download attempt {attempt} failed: {reason:?}");
            Err(reason)
        }
        Err(_) => {
            tracing::debug!("media download attempt {attempt} timed out");
            Err(MediaFailure::Download)
        }
    }
}

pub(super) fn ext_from_magic(data: &[u8]) -> &'static str {
    infer::get(data).map_or("png", |t| t.extension())
}
