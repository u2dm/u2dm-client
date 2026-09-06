use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::time::Duration;

use matrix_sdk::Client;
use matrix_sdk::media::{MediaFormat, MediaRequestParameters};
use matrix_sdk::ruma::OwnedMxcUri;
use matrix_sdk::ruma::events::room::MediaSource;
use tokio::fs;
use tokio::sync::Semaphore;
use tokio::time::{sleep, timeout};

use super::cache::{CacheHandle, FailureTracker};
use super::{
    AVATARS_DIR, STICKERS_DIR, VIDEOS_DIR, lookup_full_media_source, mxc_avatar_key, sticker_key,
    thumb_key, thumbnail_format, video_key,
};
use crate::adapters::matrix::store::purge_dir;
use crate::adapters::{container, private_fs};
use crate::domain::account::AccountScope;
use crate::domain::media::{MediaFailure, MediaResult, ThumbnailOutcome};
use crate::domain::message::TimelineMessage;
use crate::error::{AppError, Result};
use crate::ports::matrix::CleanupReport;
use crate::util::hex_encode_id;

const MAX_CONCURRENT_DOWNLOADS: usize = 6;
const MAX_CONCURRENT_FULL_DOWNLOADS: usize = 2;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);
const FULL_DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(2);
const RETRY_MAX_ATTEMPTS: u32 = 3;
const RETRY_BACKOFF_BASE: Duration = Duration::from_millis(500);
const MAX_MEDIA_BYTES: usize = 20 * 1024 * 1024;
const MAX_FULL_MEDIA_BYTES: usize = 100 * 1024 * 1024;

const MEDIA_CACHE_DIR: &str = "media-cache";
const LAYOUT_VERSION: &str = "v1";

#[derive(Clone, Copy)]
pub(crate) enum Naming {
    FromMagic,
    LauncherSafeVideo,
}

impl Naming {
    fn extension(self, data: &[u8]) -> MediaResult<&'static str> {
        match self {
            Self::FromMagic => Ok(ext_from_magic(data)),
            Self::LauncherSafeVideo => {
                container::launcher_safe_video_extension(data).ok_or(MediaFailure::Unreadable)
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct DownloadLimits {
    timeout: Duration,
    max_bytes: usize,
    full: bool,
}

impl DownloadLimits {
    pub(crate) const fn thumbnail() -> Self {
        Self {
            timeout: DOWNLOAD_TIMEOUT,
            max_bytes: MAX_MEDIA_BYTES,
            full: false,
        }
    }

    pub(crate) const fn full_file() -> Self {
        Self {
            timeout: FULL_DOWNLOAD_TIMEOUT,
            max_bytes: MAX_FULL_MEDIA_BYTES,
            full: true,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Fetch {
    format: MediaFormat,
    limits: DownloadLimits,
    naming: Naming,
}

impl Fetch {
    pub(crate) fn rendered(format: MediaFormat) -> Self {
        Self {
            format,
            limits: DownloadLimits::thumbnail(),
            naming: Naming::FromMagic,
        }
    }

    pub(crate) fn launchable_video() -> Self {
        Self {
            format: MediaFormat::File,
            limits: DownloadLimits::full_file(),
            naming: Naming::LauncherSafeVideo,
        }
    }
}

struct MediaSession {
    media_dir: PathBuf,
    cache: CacheHandle,
}

pub(crate) struct MediaService {
    root: PathBuf,
    session: StdRwLock<Option<Arc<MediaSession>>>,
    semaphore: Semaphore,
    full_semaphore: Semaphore,
    failures: StdMutex<FailureTracker>,
}

impl MediaService {
    pub(crate) fn new(cache_dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            root: cache_dir.join(MEDIA_CACHE_DIR),
            session: StdRwLock::new(None),
            semaphore: Semaphore::new(MAX_CONCURRENT_DOWNLOADS),
            full_semaphore: Semaphore::new(MAX_CONCURRENT_FULL_DOWNLOADS),
            failures: StdMutex::new(FailureTracker::default()),
        })
    }

    fn versioned_root(&self) -> PathBuf {
        self.root.join(LAYOUT_VERSION)
    }

    fn account_dir(&self, account: &AccountScope) -> PathBuf {
        self.versioned_root().join(account.id())
    }

    fn session(&self) -> Option<Arc<MediaSession>> {
        self.session.read().ok()?.clone()
    }

    pub(crate) async fn open(&self, account: &AccountScope) {
        self.detach().await;
        self.sweep(Some(account)).await;
        private_fs::restrict_existing(&self.root).await;

        let media_dir = self.account_dir(account);
        let cache = CacheHandle::spawn(media_dir.clone()).await;
        let session = Arc::new(MediaSession { media_dir, cache });
        if let Ok(mut guard) = self.session.write() {
            *guard = Some(session);
        }
    }

    pub(crate) async fn close(&self, account: &AccountScope) -> CleanupReport {
        self.detach().await;
        let mut report = CleanupReport::default();
        purge_dir(
            &self.versioned_root(),
            &self.account_dir(account),
            &mut report,
        )
        .await;
        report
    }

    async fn detach(&self) {
        if let Ok(mut failures) = self.failures.lock() {
            failures.clear();
        }
        let previous = self.session.write().ok().and_then(|mut guard| guard.take());
        let Some(session) = previous else {
            return;
        };
        session.cache.clear().await;
    }

    pub(crate) async fn sweep(&self, keep: Option<&AccountScope>) {
        remove_all_except(&self.root, Some(LAYOUT_VERSION)).await;
        remove_all_except(&self.versioned_root(), keep.map(AccountScope::id)).await;
    }

    pub(crate) async fn ensure_dirs(&self) {
        let Some(session) = self.session() else {
            return;
        };
        for dir in [
            session.media_dir.clone(),
            session.media_dir.join(AVATARS_DIR),
            session.media_dir.join(STICKERS_DIR),
            session.media_dir.join(VIDEOS_DIR),
        ] {
            if let Err(e) = private_fs::create_dir(&dir).await {
                tracing::warn!(path = %dir.display(), "failed to create media dir: {e}");
            }
        }
    }

    pub(crate) fn cache_get(&self, key: &str) -> Option<PathBuf> {
        self.session()?.cache.get(key)
    }

    pub(crate) fn is_failed(&self, key: &str) -> bool {
        self.failures.lock().is_ok_and(|f| f.should_skip(key))
    }

    pub(crate) fn failure(&self, key: &str) -> Option<MediaFailure> {
        self.failures.lock().ok()?.giving_up(key)
    }

    fn record_failure(&self, key: &str, reason: MediaFailure) {
        if let Ok(mut failures) = self.failures.lock() {
            failures.record_failure(key, reason);
        }
    }

    fn record_success(&self, key: &str) {
        if let Ok(mut failures) = self.failures.lock() {
            failures.record_success(key);
        }
    }

    async fn store(&self, key: &str, path: PathBuf, bytes: u64) {
        if let Some(session) = self.session() {
            session.cache.insert(key, path, bytes).await;
        }
    }

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
        let _permit = semaphore
            .acquire()
            .await
            .map_err(|_| MediaFailure::Download)?;

        let mut backoff = RETRY_BACKOFF_BASE;
        for attempt in 1..=RETRY_MAX_ATTEMPTS {
            if let Some(data) = attempt_download(client, request, attempt, download_timeout).await {
                if data.len() > max_bytes {
                    tracing::debug!(
                        "media payload {} bytes exceeds the {max_bytes} byte limit",
                        data.len()
                    );
                    return Err(MediaFailure::TooLarge);
                }
                return Ok(data);
            }
            if attempt < RETRY_MAX_ATTEMPTS {
                sleep(backoff).await;
                backoff = backoff.saturating_mul(2);
            }
        }
        Err(MediaFailure::Download)
    }

    pub(crate) async fn materialize_video(
        &self,
        client: &Client,
        media_sources: &StdMutex<HashMap<String, MediaSource>>,
        event_id: &str,
    ) -> MediaResult<PathBuf> {
        let cache_key = video_key(event_id);
        if let Some(cached) = self.cache_get(&cache_key) {
            return Ok(cached);
        }
        if let Some(reason) = self.failure(&cache_key) {
            return Err(reason);
        }
        let source =
            lookup_full_media_source(media_sources, event_id).ok_or(MediaFailure::NoSource)?;
        let videos = self.videos_dir().ok_or(MediaFailure::Storage)?;
        if let Err(e) = private_fs::create_dir(&videos).await {
            tracing::warn!("failed to create video dir: {e}");
            return Err(MediaFailure::Storage);
        }
        let cache_stem = videos.join(hex_encode_id(event_id));
        self.fetch_and_materialize(
            client,
            source,
            &cache_key,
            &cache_stem,
            Fetch::launchable_video(),
        )
        .await
    }

    pub(crate) async fn fetch_and_materialize(
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

    pub(crate) async fn enrich_thumbnail(
        &self,
        client: &Client,
        media_sources: &StdMutex<HashMap<String, MediaSource>>,
        msg: &TimelineMessage,
    ) -> ThumbnailOutcome {
        let Some((kind, meta)) = msg.body.media() else {
            return ThumbnailOutcome::Unchanged;
        };
        let Some(media_key) = msg.media_key() else {
            return ThumbnailOutcome::Unchanged;
        };
        let cache_key = thumb_key(media_key);

        if self.cache_get(&cache_key).is_some() {
            return ThumbnailOutcome::Unchanged;
        }

        let lane = super::lane(kind, meta);

        let materialized = match (lane.source(media_sources, media_key), self.session()) {
            (Some(source), Some(session)) => {
                let cache_stem = session.media_dir.join(hex_encode_id(media_key));
                self.fetch_and_materialize(
                    client,
                    source,
                    &cache_key,
                    &cache_stem,
                    Fetch::rendered(lane.format()),
                )
                    .await
            }
            (None, _) => Err(MediaFailure::NoSource),
            (_, None) => Err(MediaFailure::Storage),
        };

        match materialized {
            Ok(_) => ThumbnailOutcome::Ready,
            Err(reason) => ThumbnailOutcome::Failed(reason),
        }
    }

    pub(crate) async fn enrich_avatar(
        &self,
        client: &Client,
        msg: &TimelineMessage,
    ) -> Option<String> {
        let mxc = msg.sender_avatar_url.as_deref()?;
        let cache_key = mxc_avatar_key(mxc);
        if self.cache_get(&cache_key).is_some() {
            return None;
        }
        self.fetch_avatar_by_mxc(client, &cache_key, mxc.into())
            .await
            .map(|_| mxc.to_owned())
    }

    pub(crate) async fn fetch_avatar_by_mxc(
        &self,
        client: &Client,
        cache_key: &str,
        mxc: OwnedMxcUri,
    ) -> Option<PathBuf> {
        if let Some(cached) = self.cache_get(cache_key) {
            return Some(cached);
        }
        let avatars = self.avatars_dir()?;
        if let Err(e) = private_fs::create_dir(&avatars).await {
            tracing::warn!("failed to create avatar dir: {e}");
            return None;
        }
        let cache_stem = avatars.join(hex_encode_id(mxc.as_str()));
        let source = MediaSource::Plain(mxc);
        self.fetch_and_materialize(
            client,
            source,
            cache_key,
            &cache_stem,
            Fetch::rendered(thumbnail_format()),
        )
            .await
            .ok()
    }

    pub(crate) async fn fetch_sticker_by_mxc(
        &self,
        client: &Client,
        mxc: OwnedMxcUri,
    ) -> Option<PathBuf> {
        let cache_key = sticker_key(mxc.as_str());
        if let Some(cached) = self.cache_get(&cache_key) {
            return Some(cached);
        }
        let stickers = self.stickers_dir()?;
        if let Err(e) = private_fs::create_dir(&stickers).await {
            tracing::warn!("failed to create sticker dir: {e}");
            return None;
        }
        let cache_stem = stickers.join(hex_encode_id(mxc.as_str()));
        let source = MediaSource::Plain(mxc);
        self.fetch_and_materialize(
            client,
            source,
            &cache_key,
            &cache_stem,
            Fetch::rendered(MediaFormat::File),
        )
            .await
            .ok()
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

        let key = mxc_avatar_key(mxc.as_str());
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

        let (format, limits) = if thumbnail {
            (thumbnail_format(), DownloadLimits::thumbnail())
        } else {
            (MediaFormat::File, DownloadLimits::full_file())
        };
        let max_bytes = limits.max_bytes;

        let request = MediaRequestParameters { source, format };
        self.download(client, &request, limits)
            .await
            .map_err(|reason| {
                AppError::Other(format!(
                    "media download for event {event_id} failed: {reason:?} \
                     (limit {max_bytes} bytes)"
                ))
            })
    }

    pub(crate) fn needs_media_download(&self, msg: &TimelineMessage) -> bool {
        let needs_thumbnail = msg.body.media().is_some()
            && msg.media_key().is_some_and(|media_key| {
                let key = thumb_key(media_key);
                self.cache_get(&key).is_none() && !self.is_failed(&key)
            });
        let needs_avatar = msg.sender_avatar_url.as_deref().is_some_and(|mxc| {
            let key = mxc_avatar_key(mxc);
            self.cache_get(&key).is_none() && !self.is_failed(&key)
        });
        needs_thumbnail || needs_avatar
    }

    fn avatars_dir(&self) -> Option<PathBuf> {
        Some(self.session()?.media_dir.join(AVATARS_DIR))
    }

    fn stickers_dir(&self) -> Option<PathBuf> {
        Some(self.session()?.media_dir.join(STICKERS_DIR))
    }

    fn videos_dir(&self) -> Option<PathBuf> {
        Some(self.session()?.media_dir.join(VIDEOS_DIR))
    }
}

async fn remove_all_except(dir: &Path, keep: Option<&str>) {
    let Ok(mut entries) = fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_name().to_str() != keep {
            remove_entry(&entry).await;
        }
    }
}

async fn remove_entry(entry: &fs::DirEntry) {
    let path = entry.path();
    let removed = match entry.file_type().await {
        Ok(file_type) if file_type.is_dir() => fs::remove_dir_all(&path).await,
        _ => fs::remove_file(&path).await,
    };
    match removed {
        Ok(()) => tracing::info!(path = %path.display(), "removed cached media"),
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(path = %path.display(), "cached media remains: {e}"),
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
