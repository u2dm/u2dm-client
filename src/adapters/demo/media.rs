use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::{env, fs};

use super::{attachments, data, room_info, space_index, stickers};
use crate::adapters::body_emoji;
use crate::domain::media::{ContentKey, MediaFailure, MediaUsage, Waveform};
use crate::domain::message::{CustomEmoji, MessageBody};
use crate::domain::user_info::localpart;
use crate::ports::media::MediaCache;

const DATA_ENV: &str = "U2DM_DEMO_DATA";
const AUDIO_ASSET_EXTENSIONS: &[&str] = &["ogg", "m4a", "mp3"];
const IMAGE_ASSET_EXTENSIONS: &[&str] = &["gif", "webp", "png", "jpg", "heif"];
const MXC_SCHEME: &str = "mxc://";
const MEBIBYTE: u64 = 1024 * 1024;
const CLEARABLE_AT_START: u64 = 412 * MEBIBYTE;
const KEPT_BYTES: u64 = 18 * MEBIBYTE;

const FAILURE_SUFFIXES: &[(&str, MediaFailure)] = &[
    ("-missing-download", MediaFailure::Download),
    ("-missing-large", MediaFailure::TooLarge),
    ("-missing-storage", MediaFailure::Storage),
    ("-missing-unreadable", MediaFailure::Unreadable),
    ("-missing", MediaFailure::NoSource),
];

fn demo_failure(id: &str) -> Option<MediaFailure> {
    FAILURE_SUFFIXES
        .iter()
        .find(|(suffix, _)| id.ends_with(suffix))
        .map(|(_, reason)| *reason)
}

pub struct DemoMediaCache;

impl MediaCache for DemoMediaCache {
    fn thumbnail_path(&self, content: &ContentKey) -> Option<PathBuf> {
        let event_id = content.as_str();
        if let Some(sent) = attachments::preview_path(event_id) {
            return Some(sent);
        }
        if is_off_disk(content) {
            return None;
        }
        let path = match data::sticker_asset_in(event_id) {
            Some(asset) => sticker_asset_path(asset),
            None => probe("thumbnail", event_id),
        };
        if path.is_some() && AUTO_LOAD.load(Ordering::Relaxed) {
            remember_downloaded(content);
        }
        path
    }

    fn thumbnail_failure(&self, content: &ContentKey) -> Option<MediaFailure> {
        self.thumbnail_path(content)
            .is_none()
            .then(|| demo_failure(content.as_str()))
            .flatten()
    }

    fn withholds(&self, content: &ContentKey) -> bool {
        !AUTO_LOAD.load(Ordering::Relaxed)
            && demo_failure(content.as_str()) != Some(MediaFailure::NoSource)
            && attachments::preview_path(content.as_str()).is_none()
            && !DOWNLOADED_MEDIA
                .lock()
                .is_ok_and(|downloaded| downloaded.contains(content))
    }

    fn user_avatar_path(&self, mxc: &str) -> Option<PathBuf> {
        let user_id = room_info::avatar_owner(mxc)?;
        asset(&format!("avatar-{}.png", localpart(user_id)))
    }

    fn room_avatar_path(&self, mxc: &str) -> Option<PathBuf> {
        if mxc.starts_with('@') {
            return self.user_avatar_path(mxc);
        }
        if unjoined_avatar_withheld(mxc) {
            return None;
        }
        asset(&format!("room-{mxc}.png"))
    }

    fn space_avatar_path(&self, mxc: &str) -> Option<PathBuf> {
        if unjoined_avatar_withheld(mxc) {
            return None;
        }
        asset(&format!("space-{mxc}.png"))
    }

    fn sticker_path(&self, mxc: &str) -> Option<PathBuf> {
        sticker_downloaded(mxc)
            .then(|| sticker_asset_path(mxc_asset(mxc)))
            .flatten()
    }

    fn sticker_failed(&self, mxc: &str) -> bool {
        sticker_downloaded(mxc)
            && demo_failure(mxc_asset(mxc)).is_some()
            && sticker_asset_path(mxc_asset(mxc)).is_none()
    }

    fn custom_emoji_path(&self, mxc: &str) -> Option<PathBuf> {
        sticker_asset_path(mxc_asset(mxc))
    }

    fn audio_path(&self, content: &ContentKey) -> Option<PathBuf> {
        let event_id = content.as_str();
        was_fetched(event_id)
            .then(|| audio_asset_path(event_id))
            .flatten()
    }

    fn audio_failure(&self, content: &ContentKey) -> Option<MediaFailure> {
        let event_id = content.as_str();
        (was_fetched(event_id) && audio_asset_path(event_id).is_none())
            .then(|| demo_failure(event_id).unwrap_or(MediaFailure::NoSource))
    }

    fn audio_waveform(&self, content: &ContentKey) -> Option<Waveform> {
        LEARNED_WAVEFORMS
            .lock()
            .ok()?
            .get(content.as_str())
            .cloned()
    }
}

static AUTO_LOAD: AtomicBool = AtomicBool::new(true);

static DOWNLOADED_MEDIA: LazyLock<Mutex<HashSet<ContentKey>>> = LazyLock::new(Mutex::default);

pub(super) fn set_auto_load(on: bool) {
    AUTO_LOAD.store(on, Ordering::Relaxed);
}

pub(super) fn remember_downloaded(content: &ContentKey) {
    if let Ok(mut off_disk) = OFF_DISK.lock() {
        off_disk.remove(content);
    }
    if let Ok(mut downloaded) = DOWNLOADED_MEDIA.lock() {
        downloaded.insert(content.clone());
    }
}

static CLEARABLE_BYTES: AtomicU64 = AtomicU64::new(CLEARABLE_AT_START);

static CLEARED_UNSEEN: AtomicBool = AtomicBool::new(false);

static OFF_DISK: LazyLock<Mutex<HashSet<ContentKey>>> = LazyLock::new(Mutex::default);

pub(super) fn cache_usage() -> MediaUsage {
    MediaUsage {
        clearable: CLEARABLE_BYTES.load(Ordering::Relaxed),
        kept: KEPT_BYTES,
    }
}

pub(super) fn clear_cache(keep: &[PathBuf]) -> MediaUsage {
    let playing = keep
        .iter()
        .filter_map(|path| fs::metadata(path).ok())
        .map(|file| file.len())
        .sum();
    CLEARABLE_BYTES.store(playing, Ordering::Relaxed);
    if let Ok(mut downloaded) = DOWNLOADED_MEDIA.lock() {
        downloaded.clear();
    }
    if let Ok(mut fetched) = FETCHED_AUDIO.lock() {
        fetched
            .retain(|event_id| audio_asset_path(event_id).is_some_and(|path| keep.contains(&path)));
    }
    CLEARED_UNSEEN.store(true, Ordering::Relaxed);
    cache_usage()
}

pub(super) fn take_cleared() -> bool {
    CLEARED_UNSEEN.swap(false, Ordering::Relaxed)
}

pub(super) fn take_off_disk(content: &ContentKey) {
    if let Ok(mut off_disk) = OFF_DISK.lock() {
        off_disk.insert(content.clone());
    }
}

fn is_off_disk(content: &ContentKey) -> bool {
    OFF_DISK
        .lock()
        .is_ok_and(|off_disk| off_disk.contains(content))
}

static LEARNED_WAVEFORMS: LazyLock<Mutex<HashMap<String, Waveform>>> =
    LazyLock::new(Mutex::default);

pub(super) fn remember_waveform(event_id: &str, waveform: Waveform) {
    if let Ok(mut learned) = LEARNED_WAVEFORMS.lock() {
        learned.insert(event_id.to_owned(), waveform);
    }
}

static FETCHED_AUDIO: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

static PREFETCHED_PACK_IMAGES: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

fn sticker_downloaded(mxc: &str) -> bool {
    !stickers::scenario().images_trickle
        || PREFETCHED_PACK_IMAGES
            .lock()
            .is_ok_and(|prefetched| prefetched.contains(mxc))
}

pub(super) fn prefetch_pack_images(mxcs: &[String]) -> usize {
    if let Ok(mut prefetched) = PREFETCHED_PACK_IMAGES.lock() {
        prefetched.extend(mxcs.iter().cloned());
    }
    mxcs.iter()
        .filter(|mxc| sticker_asset_path(mxc_asset(mxc)).is_some())
        .count()
}

pub(super) fn custom_emoji(key: &str) -> Option<CustomEmoji> {
    if !key.starts_with(MXC_SCHEME) {
        return None;
    }
    Some(match sticker_asset_path(mxc_asset(key)) {
        Some(_) => CustomEmoji::Downloaded,
        None => CustomEmoji::Unavailable,
    })
}

pub(super) fn emoji_states(body: &MessageBody) -> BTreeMap<String, CustomEmoji> {
    let Some(html) = body.html() else {
        return BTreeMap::new();
    };
    body_emoji::emoticon_sources(html)
        .into_iter()
        .filter_map(|mxc| {
            let state = custom_emoji(&mxc)?;
            Some((mxc, state))
        })
        .collect()
}

static FETCHED_UNJOINED_AVATARS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

fn unjoined_avatar_withheld(mxc: &str) -> bool {
    space_index::scenario().is_slow
        && data::is_unjoined_avatar(mxc)
        && !FETCHED_UNJOINED_AVATARS
            .lock()
            .is_ok_and(|fetched| fetched.contains(mxc))
}

pub(super) fn fetch_unjoined_avatars(mxcs: &[String]) -> usize {
    if let Ok(mut fetched) = FETCHED_UNJOINED_AVATARS.lock() {
        fetched.extend(mxcs.iter().cloned());
    }
    mxcs.iter()
        .filter(|mxc| {
            asset(&format!("room-{mxc}.png")).is_some()
                || asset(&format!("space-{mxc}.png")).is_some()
        })
        .count()
}

pub(super) fn fetch_member_avatars(mxcs: &[String]) -> usize {
    room_info::remember_fetched_avatars(mxcs);
    mxcs.iter()
        .filter_map(|mxc| room_info::avatar_owner(mxc))
        .filter(|user_id| asset(&format!("avatar-{}.png", localpart(user_id))).is_some())
        .count()
}

fn was_fetched(event_id: &str) -> bool {
    FETCHED_AUDIO
        .lock()
        .is_ok_and(|fetched| fetched.contains(event_id))
}

pub(super) fn fetch_audio(event_id: &str) -> Option<PathBuf> {
    if let Ok(mut fetched) = FETCHED_AUDIO.lock() {
        fetched.insert(event_id.to_owned());
    }
    audio_asset_path(event_id)
}

pub fn assets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo")
}

pub fn data_path() -> PathBuf {
    match env::var_os(DATA_ENV) {
        Some(raw) => PathBuf::from(raw),
        None => assets_dir().join("data.json"),
    }
}

pub fn user_avatar_path() -> Option<PathBuf> {
    asset(&format!("avatar-{}.png", localpart(data::own_user())))
}

fn probe(prefix: &str, name: &str) -> Option<PathBuf> {
    IMAGE_ASSET_EXTENSIONS
        .iter()
        .find_map(|extension| asset(&format!("{prefix}-{name}.{extension}")))
}

pub(super) fn video_asset_path(event_id: &str) -> Option<PathBuf> {
    asset(&format!("video-{event_id}.mp4")).or_else(|| asset(&format!("video-{event_id}.webm")))
}

pub(super) fn full_file_path(event_id: &str) -> Option<PathBuf> {
    attachments::sent_file_path(event_id)
        .or_else(|| video_asset_path(event_id))
        .or_else(|| audio_asset_path(event_id))
        .or_else(|| DemoMediaCache.thumbnail_path(&content_of(event_id)))
}

fn audio_asset_path(event_id: &str) -> Option<PathBuf> {
    attachments::sent_file_path(event_id).or_else(|| {
        AUDIO_ASSET_EXTENSIONS
            .iter()
            .find_map(|extension| asset(&format!("audio-{event_id}.{extension}")))
    })
}

fn sticker_asset_path(asset: &str) -> Option<PathBuf> {
    probe("sticker", asset).or_else(|| probe("thumbnail", asset))
}

fn asset(name: &str) -> Option<PathBuf> {
    let path = assets_dir().join(name);
    path.is_file().then_some(path)
}

pub fn content_of(event_id: &str) -> ContentKey {
    ContentKey::new(event_id.to_owned())
}

pub fn mxc_asset(mxc: &str) -> &str {
    mxc.rsplit('/').next().unwrap_or(mxc)
}
