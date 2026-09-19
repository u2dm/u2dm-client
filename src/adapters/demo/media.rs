use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use super::{attachments, data, stickers};
use crate::domain::media::{ContentKey, MediaFailure, Waveform};
use crate::ports::media::MediaCache;

const DATA_ENV: &str = "U2DM_DEMO_DATA";
const AUDIO_ASSET_EXTENSIONS: &[&str] = &["ogg", "m4a", "mp3"];
const IMAGE_ASSET_EXTENSIONS: &[&str] = &["gif", "webp", "png", "jpg", "heif"];

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
        match data::sticker_asset_in(event_id) {
            Some(asset) => sticker_asset_path(asset),
            None => probe("thumbnail", event_id),
        }
    }

    fn thumbnail_failure(&self, content: &ContentKey) -> Option<MediaFailure> {
        self.thumbnail_path(content)
            .is_none()
            .then(|| demo_failure(content.as_str()))
            .flatten()
    }

    fn user_avatar_path(&self, mxc: &str) -> Option<PathBuf> {
        asset(&format!("avatar-{}.png", localpart(mxc)))
    }

    fn room_avatar_path(&self, mxc: &str) -> Option<PathBuf> {
        if mxc.starts_with('@') {
            return self.user_avatar_path(mxc);
        }
        asset(&format!("room-{mxc}.png"))
    }

    fn space_avatar_path(&self, mxc: &str) -> Option<PathBuf> {
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
        learned_waveforms()
            .lock()
            .ok()?
            .get(content.as_str())
            .cloned()
    }
}

fn learned_waveforms() -> &'static Mutex<HashMap<String, Waveform>> {
    static LEARNED: OnceLock<Mutex<HashMap<String, Waveform>>> = OnceLock::new();
    LEARNED.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn remember_waveform(event_id: &str, waveform: Waveform) {
    if let Ok(mut learned) = learned_waveforms().lock() {
        learned.insert(event_id.to_owned(), waveform);
    }
}

fn fetched_audio() -> &'static Mutex<HashSet<String>> {
    static FETCHED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    FETCHED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn prefetched_stickers() -> &'static Mutex<HashSet<String>> {
    static PREFETCHED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    PREFETCHED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn sticker_downloaded(mxc: &str) -> bool {
    !stickers::scenario().images_trickle
        || prefetched_stickers()
            .lock()
            .is_ok_and(|prefetched| prefetched.contains(mxc))
}

pub(super) fn prefetch_stickers(mxcs: &[String]) -> usize {
    if let Ok(mut prefetched) = prefetched_stickers().lock() {
        prefetched.extend(mxcs.iter().cloned());
    }
    mxcs.iter()
        .filter(|mxc| sticker_asset_path(mxc_asset(mxc)).is_some())
        .count()
}

fn was_fetched(event_id: &str) -> bool {
    fetched_audio()
        .lock()
        .is_ok_and(|fetched| fetched.contains(event_id))
}

pub(super) fn fetch_audio(event_id: &str) -> Option<PathBuf> {
    if let Ok(mut fetched) = fetched_audio().lock() {
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
    asset(&format!("video-{event_id}.mp4"))
        .or_else(|| asset(&format!("video-{event_id}.webm")))
}

fn audio_asset_path(event_id: &str) -> Option<PathBuf> {
    attachments::sent_audio_path(event_id).or_else(|| {
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

fn localpart(user_id: &str) -> &str {
    user_id
        .trim_start_matches('@')
        .split(':')
        .next()
        .unwrap_or(user_id)
}
