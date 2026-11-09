mod bounded;
mod cache;
mod flight;
mod service;
mod source;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use matrix_sdk::Client;
use matrix_sdk::media::{MediaFormat, MediaThumbnailSettings, UniqueKey};
use matrix_sdk::ruma::OwnedMxcUri;
use matrix_sdk::ruma::events::room::MediaSource;
use service::Materialized;
pub(crate) use service::{MediaService, Playable};
use sha2::{Digest, Sha256};
pub(crate) use source::EventMedia;
use tokio::task::JoinSet;

use super::session::ClientHandle;
use crate::domain::media::{
    ContentKey, ImageMeta, MediaFailure, MediaKind, MediaRendition, MediaUsage, Waveform,
    WaveformNeed,
};
use crate::domain::message::TimelineMessage;
use crate::domain::room::RoomId;
use crate::error::{AppError, Result};
use crate::ports::matrix::MediaPort;
use crate::ports::media::MediaCache;
use crate::util::hex_encode;

pub(super) const AVATARS_DIR: &str = "avatars";
pub(super) const STICKERS_DIR: &str = "stickers";
pub(super) const VIDEOS_DIR: &str = "videos";
pub(super) const VIDEO_KEY_PREFIX: &str = "video:";
pub(super) const AUDIO_DIR: &str = "audio";
pub(super) const AUDIO_KEY_PREFIX: &str = "audio:";
pub(super) const AVATAR_KEY_PREFIX: &str = "mxc-avatar:";
pub(super) const STICKER_KEY_PREFIX: &str = "sticker:";
const MAX_INFLIGHT_AVATARS: usize = 8;
const THUMBNAIL_EXTENT: u32 = 400;

pub(super) fn thumb_key(content: &ContentKey) -> String {
    format!("thumb:{}", content.as_str())
}

pub(super) fn mxc_avatar_key(mxc: &str) -> String {
    format!("{AVATAR_KEY_PREFIX}{mxc}")
}

pub(super) async fn fetch_avatar_thumbnails(
    client: &Client,
    media: &Arc<MediaService>,
    mxcs: &[String],
) -> usize {
    count_fetched(mxcs, MAX_INFLIGHT_AVATARS, |mxc| {
        let key = mxc_avatar_key(mxc);
        let uri: OwnedMxcUri = mxc.into();
        let client = client.clone();
        let media = Arc::clone(media);
        async move {
            media
                .fetch_avatar_by_mxc(&client, &key, uri)
                .await
                .is_some()
        }
    })
    .await
}

pub(super) async fn count_fetched<Fetch>(
    mxcs: &[String],
    inflight: usize,
    mut fetch: impl FnMut(&str) -> Fetch + Send,
) -> usize
where
    Fetch: Future<Output = bool> + Send + 'static,
{
    let mut fetched = 0;
    let mut tasks: JoinSet<bool> = JoinSet::new();
    for mxc in mxcs {
        tasks.spawn(fetch(mxc));
        if tasks.len() >= inflight {
            fetched += usize::from(matches!(tasks.join_next().await, Some(Ok(true))));
        }
    }
    while let Some(result) = tasks.join_next().await {
        fetched += usize::from(matches!(result, Ok(true)));
    }
    fetched
}

pub(super) fn video_key(content: &ContentKey) -> String {
    format!("{VIDEO_KEY_PREFIX}{}", content.as_str())
}

pub(super) fn audio_key(content: &ContentKey) -> String {
    format!("{AUDIO_KEY_PREFIX}{}", content.as_str())
}

pub(super) fn content_key(source: &MediaSource, format: &MediaFormat) -> ContentKey {
    let declared = serde_json::to_vec(source).unwrap_or_else(|e| {
        tracing::warn!("identifying a media source by its url alone: {e}");
        source.unique_key().into_bytes()
    });
    let mut hasher = Sha256::new();
    for part in [format.unique_key().as_bytes(), declared.as_slice()] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    ContentKey::new(hex_encode(&hasher.finalize()))
}

pub(crate) fn file_content(source: &MediaSource) -> ContentKey {
    content_key(source, &MediaFormat::File)
}

pub(crate) fn thumbnail_content(
    kind: MediaKind,
    meta: &ImageMeta,
    media: EventMedia,
) -> Option<ContentKey> {
    ThumbnailRequest::pick(kind, meta, media).map(|request| request.content)
}

fn sticker_key(mxc: &str) -> String {
    format!("{STICKER_KEY_PREFIX}{mxc}")
}

pub(super) struct MaterializedMedia {
    service: Arc<MediaService>,
}

impl MaterializedMedia {
    pub(super) fn new(service: Arc<MediaService>) -> Self {
        Self { service }
    }
}

impl MediaCache for MaterializedMedia {
    fn thumbnail_path(&self, content: &ContentKey) -> Option<PathBuf> {
        self.service.cache_get(&thumb_key(content))
    }

    fn thumbnail_failure(&self, content: &ContentKey) -> Option<MediaFailure> {
        self.service.failure(&thumb_key(content))
    }

    fn withholds(&self, content: &ContentKey) -> bool {
        self.service.withholds(content)
    }

    fn user_avatar_path(&self, mxc: &str) -> Option<PathBuf> {
        self.service.cache_get(&mxc_avatar_key(mxc))
    }

    fn room_avatar_path(&self, mxc: &str) -> Option<PathBuf> {
        self.service.cache_get(&mxc_avatar_key(mxc))
    }

    fn space_avatar_path(&self, mxc: &str) -> Option<PathBuf> {
        self.service.cache_get(&mxc_avatar_key(mxc))
    }

    fn sticker_path(&self, mxc: &str) -> Option<PathBuf> {
        self.service.cache_get(&sticker_key(mxc))
    }

    fn sticker_failed(&self, mxc: &str) -> bool {
        self.service.is_failed(&sticker_key(mxc))
    }

    fn custom_emoji_path(&self, mxc: &str) -> Option<PathBuf> {
        self.service.cache_get(&sticker_key(mxc))
    }

    fn audio_path(&self, content: &ContentKey) -> Option<PathBuf> {
        self.service.cache_get(&audio_key(content))
    }

    fn audio_failure(&self, content: &ContentKey) -> Option<MediaFailure> {
        self.service.failure(&audio_key(content))
    }

    fn audio_waveform(&self, content: &ContentKey) -> Option<Waveform> {
        self.service.waveform(content)
    }
}

pub(super) fn is_animated_mime(mimetype: Option<&str>) -> bool {
    mimetype.is_some_and(|mime| {
        mime.eq_ignore_ascii_case("image/gif") || mime.eq_ignore_ascii_case("image/webp")
    })
}

pub(super) fn thumbnail_format() -> MediaFormat {
    MediaFormat::Thumbnail(MediaThumbnailSettings::new(
        THUMBNAIL_EXTENT.into(),
        THUMBNAIL_EXTENT.into(),
    ))
}

#[derive(Clone, Copy)]
pub(super) enum MediaLane {
    Thumbnail,
    Poster,
    FullFile,
}

fn lane(kind: MediaKind, meta: &ImageMeta) -> MediaLane {
    match kind {
        MediaKind::Sticker => MediaLane::FullFile,
        MediaKind::Video => MediaLane::Poster,
        MediaKind::Photo if is_animated_mime(meta.mimetype.as_deref()) => MediaLane::FullFile,
        MediaKind::Photo => MediaLane::Thumbnail,
    }
}

pub(super) struct ThumbnailRequest {
    pub(super) content: ContentKey,
    lane: MediaLane,
    source: MediaSource,
}

impl ThumbnailRequest {
    pub(super) fn of(msg: &TimelineMessage, media: Option<EventMedia>) -> Option<Self> {
        let (kind, meta) = msg.body.media()?;
        Self::pick(kind, meta, media?)
    }

    fn pick(kind: MediaKind, meta: &ImageMeta, media: EventMedia) -> Option<Self> {
        let lane = lane(kind, meta);
        let source = lane.pick(media)?;
        Some(Self {
            content: content_key(&source, &lane.format()),
            lane,
            source,
        })
    }
}

impl MediaLane {
    pub(super) fn pick(self, media: EventMedia) -> Option<MediaSource> {
        let EventMedia { file, thumbnail } = media;
        match self {
            Self::FullFile => Some(file),
            Self::Thumbnail => Some(thumbnail.unwrap_or(file)),
            Self::Poster => thumbnail,
        }
    }

    pub(super) fn format(self) -> MediaFormat {
        match self {
            Self::FullFile => MediaFormat::File,
            Self::Thumbnail | Self::Poster => thumbnail_format(),
        }
    }
}

pub(super) struct MatrixMedia {
    matrix: Arc<ClientHandle>,
}

impl MatrixMedia {
    pub(super) fn new(matrix: Arc<ClientHandle>) -> Self {
        Self { matrix }
    }

    async fn materialize(
        &self,
        room_id: &RoomId,
        event_id: &str,
        playable: Playable,
    ) -> Result<Materialized> {
        let room = self.matrix.room(room_id).await?;
        self.matrix
            .media()
            .materialize_playable(&room, event_id, playable)
            .await
            .map_err(|reason| {
                AppError::Other(format!(
                    "{} download for event {event_id} failed: {reason:?}",
                    playable.noun()
                ))
            })
    }
}

fn media_cache_stopped() -> AppError {
    AppError::Other("the media cache is not running".to_owned())
}

#[async_trait]
impl MediaPort for MatrixMedia {
    fn set_auto_load(&self, on: bool) {
        self.matrix.media().set_auto_load(on);
    }

    async fn cache_usage(&self) -> Result<MediaUsage> {
        self.matrix
            .media()
            .usage()
            .await
            .ok_or_else(media_cache_stopped)
    }

    async fn clear_cache(&self, keep: Vec<PathBuf>) -> Result<MediaUsage> {
        let cleared = self
            .matrix
            .media()
            .clear_downloaded(keep)
            .await
            .ok_or_else(media_cache_stopped)?;
        if cleared.left_behind > 0 {
            return Err(AppError::Other(format!(
                "{} downloaded files could not be removed",
                cleared.left_behind
            )));
        }
        Ok(cleared.usage)
    }

    async fn download_media(
        &self,
        room_id: &RoomId,
        event_id: &str,
        rendition: MediaRendition,
    ) -> Result<Vec<u8>> {
        let room = self.matrix.room(room_id).await?;
        self.matrix
            .media()
            .download_media(&room, event_id, rendition)
            .await
    }

    async fn materialize_video(&self, room_id: &RoomId, event_id: &str) -> Result<PathBuf> {
        Ok(self
            .materialize(room_id, event_id, Playable::Video)
            .await?
            .path)
    }

    async fn materialize_audio(
        &self,
        room_id: &RoomId,
        event_id: &str,
        need: WaveformNeed,
    ) -> Result<PathBuf> {
        let Materialized { content, path } =
            self.materialize(room_id, event_id, Playable::Audio).await?;
        if need == WaveformNeed::Compute {
            self.matrix.media().learn_waveform(&content, &path).await;
        }
        Ok(path)
    }
}
