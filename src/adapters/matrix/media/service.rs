mod downloads;

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::time::Duration;

use matrix_sdk::Client;
use matrix_sdk::media::MediaFormat;
use matrix_sdk::ruma::OwnedMxcUri;
use matrix_sdk::ruma::events::room::MediaSource;
use tokio::fs;
use tokio::sync::Semaphore;
use tokio::task::spawn_blocking;

use self::downloads::{
    DOWNLOAD_TIMEOUT, FULL_DOWNLOAD_TIMEOUT, MAX_CONCURRENT_DOWNLOADS,
    MAX_CONCURRENT_FULL_DOWNLOADS, ext_from_magic,
};
use super::cache::{CacheHandle, Cleared, FailureTracker};
use super::flight::SingleFlight;
use super::{
    AUDIO_DIR, AVATARS_DIR, STICKERS_DIR, ThumbnailRequest, VIDEOS_DIR, audio_key, mxc_avatar_key,
    sticker_key, thumb_key, thumbnail_format, video_key,
};
use crate::adapters::matrix::store::purge_dir;
use crate::adapters::{container, private_fs, video};
use crate::domain::account::AccountScope;
use crate::domain::media::{
    ContentKey, MediaFailure, MediaResult, MediaUsage, ThumbnailOutcome, Waveform,
};
use crate::ports::matrix::CleanupReport;
use crate::util::hex_encode_id;

const MAX_MEDIA_BYTES: usize = 20 * 1024 * 1024;

const MAX_FULL_MEDIA_BYTES: usize = 100 * 1024 * 1024;

const MEDIA_CACHE_DIR: &str = "media-cache";

const LAYOUT_VERSION: &str = "v2";

#[derive(Clone, Copy)]
pub(crate) enum Naming {
    FromMagic,
    LauncherSafeVideo,
    LauncherSafeAudio,
}

impl Naming {
    fn extension(self, data: &[u8]) -> MediaResult<&'static str> {
        match self {
            Self::FromMagic => Ok(ext_from_magic(data)),
            Self::LauncherSafeVideo => {
                container::launcher_safe_video_extension(data).ok_or(MediaFailure::Unreadable)
            }
            Self::LauncherSafeAudio => {
                container::launcher_safe_audio_extension(data).ok_or(MediaFailure::Unreadable)
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

struct MxcTarget<'a> {
    dir: &'static str,
    noun: &'static str,
    cache_key: &'a str,
    fetch: Fetch,
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

    pub(crate) fn launchable(playable: Playable) -> Self {
        Self {
            format: MediaFormat::File,
            limits: DownloadLimits::full_file(),
            naming: match playable {
                Playable::Video => Naming::LauncherSafeVideo,
                Playable::Audio => Naming::LauncherSafeAudio,
            },
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Playable {
    Video,
    Audio,
}

impl Playable {
    pub(crate) fn noun(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Audio => "audio",
        }
    }

    fn key(self, content: &ContentKey) -> String {
        match self {
            Self::Video => video_key(content),
            Self::Audio => audio_key(content),
        }
    }

    fn dir(self) -> &'static str {
        match self {
            Self::Video => VIDEOS_DIR,
            Self::Audio => AUDIO_DIR,
        }
    }
}

pub(crate) struct Materialized {
    pub(crate) content: ContentKey,
    pub(crate) path: PathBuf,
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
    flights: SingleFlight,
    failures: StdMutex<FailureTracker>,
    waveforms: StdMutex<HashMap<ContentKey, Waveform>>,
    auto_load: AtomicBool,
    admitted: StdMutex<HashSet<ContentKey>>,
}

impl MediaService {
    pub(crate) fn new(cache_dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            root: cache_dir.join(MEDIA_CACHE_DIR),
            session: StdRwLock::new(None),
            semaphore: Semaphore::new(MAX_CONCURRENT_DOWNLOADS),
            full_semaphore: Semaphore::new(MAX_CONCURRENT_FULL_DOWNLOADS),
            flights: SingleFlight::default(),
            failures: StdMutex::new(FailureTracker::default()),
            waveforms: StdMutex::new(HashMap::new()),
            auto_load: AtomicBool::new(true),
            admitted: StdMutex::new(HashSet::new()),
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

    fn forget_failures_and_admissions(&self) {
        if let Ok(mut failures) = self.failures.lock() {
            failures.clear();
        }
        if let Ok(mut admitted) = self.admitted.lock() {
            admitted.clear();
        }
    }

    pub(crate) async fn detach(&self) {
        self.flights.clear();
        self.forget_failures_and_admissions();
        if let Ok(mut waveforms) = self.waveforms.lock() {
            waveforms.clear();
        }
        let previous = self.session.write().ok().and_then(|mut guard| guard.take());
        let Some(session) = previous else {
            return;
        };
        session.cache.stop().await;
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
            session.media_dir.join(AUDIO_DIR),
        ] {
            if let Err(e) = private_fs::create_dir(&dir).await {
                tracing::warn!(path = %dir.display(), "failed to create media dir: {e}");
            }
        }
    }

    pub(crate) fn waveform(&self, content: &ContentKey) -> Option<Waveform> {
        self.waveforms.lock().ok()?.get(content).cloned()
    }

    pub(crate) async fn learn_waveform(&self, content: &ContentKey, path: &Path) {
        if self.waveform(content).is_some() {
            return;
        }
        let owned = path.to_path_buf();
        let learned = spawn_blocking(move || video::probe_audio(&owned))
            .await
            .ok()
            .flatten()
            .and_then(|probe| probe.waveform);
        if let Some(waveform) = learned
            && let Ok(mut waveforms) = self.waveforms.lock()
        {
            waveforms.insert(content.clone(), waveform);
        }
    }

    pub(crate) async fn usage(&self) -> Option<MediaUsage> {
        self.session()?.cache.usage().await
    }

    pub(crate) async fn clear_downloaded(&self, keep: Vec<PathBuf>) -> Option<Cleared> {
        let cleared = self
            .session()?
            .cache
            .clear(keep.into_iter().collect())
            .await?;
        self.forget_failures_and_admissions();
        Some(cleared)
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

    pub(crate) async fn enrich_thumbnail(
        &self,
        client: &Client,
        request: &ThumbnailRequest,
    ) -> ThumbnailOutcome {
        let ThumbnailRequest {
            content,
            lane,
            source,
        } = request;
        let cache_key = thumb_key(content);
        let materialized = match self.session() {
            Some(session) => {
                let cache_stem = session.media_dir.join(content.as_str());
                self.fetch_and_materialize(
                    client,
                    source.clone(),
                    &cache_key,
                    &cache_stem,
                    Fetch::rendered(lane.format()),
                )
                .await
            }
            None => Err(MediaFailure::Storage),
        };

        match materialized {
            Ok(_) => ThumbnailOutcome::Ready,
            Err(reason) => ThumbnailOutcome::Failed(reason),
        }
    }

    pub(crate) async fn enrich_avatar(&self, client: &Client, mxc: &str) -> Option<String> {
        self.fetch_avatar_by_mxc(client, &mxc_avatar_key(mxc), mxc.into())
            .await
            .map(|_| mxc.to_owned())
    }

    pub(crate) async fn fetch_avatar_by_mxc(
        &self,
        client: &Client,
        cache_key: &str,
        mxc: OwnedMxcUri,
    ) -> Option<PathBuf> {
        let into = MxcTarget {
            dir: AVATARS_DIR,
            noun: "avatar",
            cache_key,
            fetch: Fetch::rendered(thumbnail_format()),
        };
        self.fetch_mxc_into(client, mxc, into).await
    }

    pub(crate) async fn fetch_sticker_by_mxc(
        &self,
        client: &Client,
        mxc: OwnedMxcUri,
    ) -> Option<PathBuf> {
        let cache_key = sticker_key(mxc.as_str());
        let into = MxcTarget {
            dir: STICKERS_DIR,
            noun: "sticker",
            cache_key: &cache_key,
            fetch: Fetch::rendered(MediaFormat::File),
        };
        self.fetch_mxc_into(client, mxc, into).await
    }

    async fn fetch_mxc_into(
        &self,
        client: &Client,
        mxc: OwnedMxcUri,
        into: MxcTarget<'_>,
    ) -> Option<PathBuf> {
        if let Some(cached) = self.cache_get(into.cache_key) {
            return Some(cached);
        }
        let dir = self.session()?.media_dir.join(into.dir);
        if let Err(e) = private_fs::create_dir(&dir).await {
            tracing::warn!("failed to create {} dir: {e}", into.noun);
            return None;
        }
        let cache_stem = dir.join(hex_encode_id(mxc.as_str()));
        let source = MediaSource::Plain(mxc);
        self.fetch_and_materialize(client, source, into.cache_key, &cache_stem, into.fetch)
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

    pub(crate) fn needs_thumbnail_download(&self, content: &ContentKey) -> bool {
        self.needs_download(&thumb_key(content))
    }

    pub(crate) fn set_auto_load(&self, on: bool) {
        self.auto_load.store(on, Ordering::Relaxed);
    }

    pub(crate) fn admit(&self, content: ContentKey) {
        if let Ok(mut admitted) = self.admitted.lock() {
            admitted.insert(content);
        }
    }

    pub(crate) fn withholds(&self, content: &ContentKey) -> bool {
        !self.auto_load.load(Ordering::Relaxed)
            && self.cache_get(&thumb_key(content)).is_none()
            && !self
                .admitted
                .lock()
                .is_ok_and(|admitted| admitted.contains(content))
    }

    pub(crate) fn needs_avatar_download(&self, mxc: &str) -> bool {
        self.needs_download(&mxc_avatar_key(mxc))
    }

    pub(crate) fn has_sticker(&self, mxc: &str) -> bool {
        self.cache_get(&sticker_key(mxc)).is_some()
    }

    fn needs_download(&self, cache_key: &str) -> bool {
        self.cache_get(cache_key).is_none() && !self.is_failed(cache_key)
    }

    fn playable_dir(&self, playable: Playable) -> Option<PathBuf> {
        Some(self.session()?.media_dir.join(playable.dir()))
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
