mod cache;
mod flight;
mod service;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use matrix_sdk::media::{MediaFormat, MediaThumbnailSettings};
use matrix_sdk::ruma::events::room::MediaSource;
pub(crate) use service::{MediaService, Playable};

use super::session::ClientHandle;
use crate::domain::media::{
    ImageMeta, MediaFailure, MediaKind, MediaRendition, Waveform, WaveformNeed,
};
use crate::domain::message::TimelineMessage;
use crate::error::{AppError, Result};
use crate::ports::matrix::MediaPort;
use crate::ports::media::MediaCache;

pub(super) type MediaSources = StdMutex<HashMap<String, MediaSource>>;

pub(super) const AVATARS_DIR: &str = "avatars";
pub(super) const STICKERS_DIR: &str = "stickers";
pub(super) const VIDEOS_DIR: &str = "videos";
pub(super) const VIDEO_KEY_PREFIX: &str = "video:";
pub(super) const AUDIO_DIR: &str = "audio";
pub(super) const AUDIO_KEY_PREFIX: &str = "audio:";

pub(super) fn thumb_key(event_id: &str) -> String {
    format!("thumb:{event_id}")
}

pub(super) fn mxc_avatar_key(mxc: &str) -> String {
    format!("mxc-avatar:{mxc}")
}

pub(super) fn video_key(event_id: &str) -> String {
    format!("{VIDEO_KEY_PREFIX}{event_id}")
}

pub(super) fn audio_key(event_id: &str) -> String {
    format!("{AUDIO_KEY_PREFIX}{event_id}")
}

fn sticker_key(mxc: &str) -> String {
    format!("sticker:{mxc}")
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
    fn thumbnail_path(&self, event_id: &str) -> Option<PathBuf> {
        self.service.cache_get(&thumb_key(event_id))
    }

    fn thumbnail_failure(&self, event_id: &str) -> Option<MediaFailure> {
        self.service.failure(&thumb_key(event_id))
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

    fn audio_path(&self, event_id: &str) -> Option<PathBuf> {
        self.service.cache_get(&audio_key(event_id))
    }

    fn audio_failure(&self, event_id: &str) -> Option<MediaFailure> {
        self.service.failure(&audio_key(event_id))
    }

    fn audio_waveform(&self, event_id: &str) -> Option<Waveform> {
        self.service.waveform(event_id)
    }
}

pub(super) fn lookup_media_source(
    media_sources: &MediaSources,
    event_id: &str,
) -> Option<MediaSource> {
    let thumb_key = format!("{event_id}:thumb");
    media_sources.lock().ok().and_then(|sources| {
        sources
            .get(&thumb_key)
            .or_else(|| sources.get(event_id))
            .cloned()
    })
}

pub(super) fn lookup_poster_source(
    media_sources: &MediaSources,
    event_id: &str,
) -> Option<MediaSource> {
    let thumb_key = format!("{event_id}:thumb");
    media_sources.lock().ok()?.get(&thumb_key).cloned()
}

pub(super) fn lookup_full_media_source(
    media_sources: &MediaSources,
    event_id: &str,
) -> Option<MediaSource> {
    media_sources.lock().ok()?.get(event_id).cloned()
}

pub(super) fn is_animated_mime(mimetype: Option<&str>) -> bool {
    mimetype.is_some_and(|mime| {
        mime.eq_ignore_ascii_case("image/gif") || mime.eq_ignore_ascii_case("image/webp")
    })
}

pub(super) fn thumbnail_format() -> MediaFormat {
    MediaFormat::Thumbnail(MediaThumbnailSettings::new(400u32.into(), 400u32.into()))
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
    pub(super) media_key: String,
    lane: MediaLane,
}

impl ThumbnailRequest {
    pub(super) fn of(msg: &TimelineMessage) -> Option<Self> {
        let (kind, meta) = msg.body.media()?;
        Some(Self {
            media_key: msg.media_key()?.to_owned(),
            lane: lane(kind, meta),
        })
    }
}

impl MediaLane {
    pub(super) fn source(
        self,
        media_sources: &MediaSources,
        event_id: &str,
    ) -> Option<MediaSource> {
        match self {
            Self::FullFile => lookup_full_media_source(media_sources, event_id),
            Self::Thumbnail => lookup_media_source(media_sources, event_id),
            Self::Poster => lookup_poster_source(media_sources, event_id),
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
    sources: Arc<MediaSources>,
}

impl MatrixMedia {
    pub(super) fn new(matrix: Arc<ClientHandle>, sources: Arc<MediaSources>) -> Self {
        Self { matrix, sources }
    }

    async fn materialize(&self, event_id: &str, playable: Playable) -> Result<PathBuf> {
        let client = self.matrix.client().await?;
        self.matrix
            .media()
            .materialize_playable(&client, &self.sources, event_id, playable)
            .await
            .map_err(|reason| {
                AppError::Other(format!(
                    "{} download for event {event_id} failed: {reason:?}",
                    playable.noun()
                ))
            })
    }
}

#[async_trait]
impl MediaPort for MatrixMedia {
    async fn download_media(&self, event_id: &str, rendition: MediaRendition) -> Result<Vec<u8>> {
        let client = self.matrix.client().await?;
        self.matrix
            .media()
            .download_media(&client, &self.sources, event_id, rendition)
            .await
    }

    async fn materialize_video(&self, event_id: &str) -> Result<PathBuf> {
        self.materialize(event_id, Playable::Video).await
    }

    async fn materialize_audio(&self, event_id: &str, need: WaveformNeed) -> Result<PathBuf> {
        let path = self.materialize(event_id, Playable::Audio).await?;
        if need == WaveformNeed::Compute {
            self.matrix.media().learn_waveform(event_id, &path).await;
        }
        Ok(path)
    }
}
