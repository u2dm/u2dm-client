use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use matrix_sdk::Client;
use matrix_sdk::media::{MediaFormat, MediaRequestParameters, MediaThumbnailSettings};
use matrix_sdk::ruma::OwnedMxcUri;
use matrix_sdk::ruma::events::room::MediaSource;
use tokio::fs;
use tokio::time::timeout;

use crate::domain::models::{MessageBody, TimelineMessage};
use crate::error::{AppError, Result};
use crate::ports::media::MediaCache;
use crate::util::hex_encode_id;

pub(super) fn thumb_key(event_id: &str) -> String {
    format!("thumb:{event_id}")
}

pub(super) fn avatar_key(sender: &str) -> String {
    format!("avatar:{sender}")
}

pub(super) fn space_avatar_key(mxc: &str) -> String {
    format!("space-avatar:{mxc}")
}

pub(super) struct MaterializedMedia {
    materialized: Arc<StdMutex<HashMap<String, PathBuf>>>,
}

impl MaterializedMedia {
    pub(super) fn new(materialized: Arc<StdMutex<HashMap<String, PathBuf>>>) -> Self {
        Self { materialized }
    }
}

impl MediaCache for MaterializedMedia {
    fn thumbnail_path(&self, event_id: &str) -> Option<PathBuf> {
        lookup_materialized(&self.materialized, &thumb_key(event_id))
    }

    fn avatar_path(&self, sender: &str) -> Option<PathBuf> {
        lookup_materialized(&self.materialized, &avatar_key(sender))
    }

    fn space_avatar_path(&self, mxc: &str) -> Option<PathBuf> {
        lookup_materialized(&self.materialized, &space_avatar_key(mxc))
    }
}

pub(super) fn lookup_media_source(
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
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

fn ext_from_magic(data: &[u8]) -> &'static str {
    infer::get(data).map_or("png", |t| t.extension())
}

pub(super) fn lookup_materialized(
    materialized: &StdMutex<HashMap<String, PathBuf>>,
    key: &str,
) -> Option<PathBuf> {
    materialized.lock().ok()?.get(key).cloned()
}

fn record_materialized(materialized: &StdMutex<HashMap<String, PathBuf>>, key: &str, path: &Path) {
    if let Ok(mut map) = materialized.lock() {
        map.insert(key.to_string(), path.to_path_buf());
    }
}

pub(super) async fn fetch_and_materialize(
    client: &Client,
    materialized: &StdMutex<HashMap<String, PathBuf>>,
    cache_stem: &Path,
    source: MediaSource,
    cache_key: &str,
) -> Option<PathBuf> {
    let format = MediaFormat::Thumbnail(MediaThumbnailSettings::new(400u32.into(), 400u32.into()));
    let request = MediaRequestParameters { source, format };

    let media = client.media();
    let download = media.get_media_content(&request, true);
    let data = match timeout(Duration::from_secs(60), download).await {
        Ok(Ok(data)) => data,
        Ok(Err(e)) => {
            tracing::debug!("thumbnail download failed for {cache_key}: {e}");
            return None;
        }
        Err(_) => {
            tracing::debug!("thumbnail download timed out for {cache_key}");
            return None;
        }
    };

    let cache_path = cache_stem.with_extension(ext_from_magic(&data));

    if let Err(e) = fs::write(&cache_path, &data).await {
        tracing::warn!("failed to write materialized media: {e}");
        return None;
    }

    record_materialized(materialized, cache_key, &cache_path);
    Some(cache_path)
}

pub(super) fn needs_media_download(
    msg: &TimelineMessage,
    materialized: &StdMutex<HashMap<String, PathBuf>>,
) -> bool {
    let needs_thumbnail = matches!(&msg.body, MessageBody::Image { .. })
        && lookup_materialized(materialized, &thumb_key(&msg.event_id.0)).is_none();
    let needs_avatar = msg.sender_avatar_url.is_some()
        && lookup_materialized(materialized, &avatar_key(&msg.sender)).is_none();
    needs_thumbnail || needs_avatar
}

pub(super) async fn enrich_message(
    client: &Client,
    media_dir: &Path,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
    materialized: &StdMutex<HashMap<String, PathBuf>>,
    msg: &TimelineMessage,
) {
    if matches!(&msg.body, MessageBody::Image { .. }) {
        let event_id = &msg.event_id.0;
        let cache_key = thumb_key(event_id);

        if lookup_materialized(materialized, &cache_key).is_none()
            && let Some(source) = lookup_media_source(media_sources, event_id)
        {
            let cache_stem = media_dir.join(hex_encode_id(event_id));
            fetch_and_materialize(client, materialized, &cache_stem, source, &cache_key).await;
        }
    }

    if let Some(mxc_url) = &msg.sender_avatar_url {
        let avatar_key = avatar_key(&msg.sender);

        if lookup_materialized(materialized, &avatar_key).is_none() {
            let avatar_dir = media_dir.join("avatars");
            let cache_stem = avatar_dir.join(hex_encode_id(&msg.sender));
            let avatar_mxc: OwnedMxcUri = mxc_url.as_str().into();
            let source = MediaSource::Plain(avatar_mxc);
            fetch_and_materialize(client, materialized, &cache_stem, source, &avatar_key).await;
        }
    }
}

pub(super) async fn fetch_user_avatar(
    client: &Client,
    media_dir: &Path,
    materialized: &StdMutex<HashMap<String, PathBuf>>,
) -> Option<PathBuf> {
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
    if let Some(path) = lookup_materialized(materialized, &key) {
        return Some(path);
    }

    let avatar_dir = media_dir.join("avatars");
    if let Err(e) = fs::create_dir_all(&avatar_dir).await {
        tracing::warn!("failed to create avatar dir: {e}");
        return None;
    }

    let cache_stem = avatar_dir.join(hex_encode_id(mxc.as_str()));
    let source = MediaSource::Plain(mxc);
    fetch_and_materialize(client, materialized, &cache_stem, source, &key).await
}

pub(super) async fn download_media(
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

    let format = if thumbnail {
        MediaFormat::Thumbnail(MediaThumbnailSettings::new(400u32.into(), 400u32.into()))
    } else {
        MediaFormat::File
    };

    let request = MediaRequestParameters { source, format };
    let data = client
        .media()
        .get_media_content(&request, true)
        .await
        .map_err(|e| AppError::Other(format!("media download failed: {e}")))?;

    Ok(data)
}
