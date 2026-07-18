mod cache;
mod service;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use matrix_sdk::media::{MediaFormat, MediaThumbnailSettings};
use matrix_sdk::ruma::events::room::MediaSource;
pub(crate) use service::MediaService;

use crate::ports::media::MediaCache;

pub(super) fn thumb_key(event_id: &str) -> String {
    format!("thumb:{event_id}")
}

pub(super) fn avatar_key(sender: &str) -> String {
    format!("avatar:{sender}")
}

pub(super) fn mxc_avatar_key(mxc: &str) -> String {
    format!("mxc-avatar:{mxc}")
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

    fn thumbnail_failed(&self, event_id: &str) -> bool {
        self.service.is_failed(&thumb_key(event_id))
    }

    fn avatar_path(&self, sender: &str) -> Option<PathBuf> {
        self.service.cache_get(&avatar_key(sender))
    }

    fn room_avatar_path(&self, mxc: &str) -> Option<PathBuf> {
        self.service.cache_get(&mxc_avatar_key(mxc))
    }

    fn space_avatar_path(&self, mxc: &str) -> Option<PathBuf> {
        self.service.cache_get(&mxc_avatar_key(mxc))
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

pub(super) fn lookup_full_media_source(
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
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
