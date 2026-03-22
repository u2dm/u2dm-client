use std::collections::HashMap;
use std::fs as fs_std;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use matrix_sdk::Client;
use matrix_sdk::media::{MediaFormat, MediaRequestParameters, MediaThumbnailSettings};
use matrix_sdk::ruma::OwnedMxcUri;
use matrix_sdk::ruma::events::room::MediaSource;
use tokio::fs;
use tokio::time::timeout;

use crate::domain::models::{MessageBody, TimelineMessage};
use crate::error::{AppError, Result};
use crate::util::hex_encode_id;

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

pub(super) fn find_cached(stem: &Path) -> Option<PathBuf> {
    let parent = stem.parent()?;
    let file_stem = stem.file_name()?;
    fs_std::read_dir(parent).ok()?.find_map(|entry| {
        let path = entry.ok()?.path();
        (path.file_stem() == Some(file_stem)).then_some(path)
    })
}

pub(super) async fn fetch_single_thumbnail(
    client: &Client,
    cache_stem: &Path,
    source: MediaSource,
    event_id: &str,
) -> Option<PathBuf> {
    let format = MediaFormat::Thumbnail(MediaThumbnailSettings::new(400u32.into(), 400u32.into()));
    let request = MediaRequestParameters { source, format };

    let media = client.media();
    let download = media.get_media_content(&request, true);
    let data = match timeout(Duration::from_secs(5), download).await {
        Ok(Ok(data)) => data,
        Ok(Err(e)) => {
            tracing::debug!("thumbnail download failed for {event_id}: {e}");
            return None;
        }
        Err(_) => {
            tracing::debug!("thumbnail download timed out for {event_id}");
            return None;
        }
    };

    let cache_path = cache_stem.with_extension(ext_from_magic(&data));

    if let Err(e) = fs::write(&cache_path, &data).await {
        tracing::warn!("failed to cache thumbnail: {e}");
        return None;
    }
    Some(cache_path)
}

pub(super) async fn enrich_message(
    client: &Client,
    cache_dir: &Path,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
    msg: &mut TimelineMessage,
) {
    if let MessageBody::Image { meta, .. } = &mut msg.body {
        let event_id = &msg.event_id.0;
        let cache_stem = cache_dir.join(hex_encode_id(event_id));

        if let Some(path) = find_cached(&cache_stem) {
            meta.thumbnail_path = Some(path);
        } else if let Some(source) = lookup_media_source(media_sources, event_id)
            && let Some(path) = fetch_single_thumbnail(client, &cache_stem, source, event_id).await
        {
            meta.thumbnail_path = Some(path);
        }
    }

    if let Some(mxc_url) = &msg.sender_avatar_url {
        let avatar_dir = cache_dir.join("avatars");
        let cache_stem = avatar_dir.join(hex_encode_id(&msg.sender));

        if let Some(path) = find_cached(&cache_stem) {
            msg.sender_avatar_path = Some(path);
        } else {
            let avatar_mxc: OwnedMxcUri = mxc_url.as_str().into();
            let source = MediaSource::Plain(avatar_mxc);
            if let Some(path) =
                fetch_single_thumbnail(client, &cache_stem, source, &msg.sender).await
            {
                msg.sender_avatar_path = Some(path);
            }
        }
    }
}

pub(super) async fn enrich_messages(
    client: &Client,
    cache_dir: &Path,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
    messages: &mut [TimelineMessage],
) {
    if let Err(e) = fs::create_dir_all(cache_dir).await {
        tracing::warn!("failed to create media cache dir: {e}");
        return;
    }
    let avatar_dir = cache_dir.join("avatars");
    if let Err(e) = fs::create_dir_all(&avatar_dir).await {
        tracing::warn!("failed to create avatar cache dir: {e}");
        return;
    }

    for msg in messages.iter_mut() {
        enrich_message(client, cache_dir, media_sources, msg).await;
    }
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
