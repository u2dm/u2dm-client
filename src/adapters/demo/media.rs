use std::path::{Path, PathBuf};

use super::{attachments, data};
use crate::domain::media::MediaFailure;
use crate::ports::media::MediaCache;

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
    fn thumbnail_path(&self, event_id: &str) -> Option<PathBuf> {
        if let Some(sent) = attachments::preview_path(event_id) {
            return Some(sent);
        }
        match data::sticker_asset_in(event_id) {
            Some(asset) => sticker_asset_path(asset),
            None => probe("thumbnail", event_id),
        }
    }

    fn thumbnail_failure(&self, event_id: &str) -> Option<MediaFailure> {
        self.thumbnail_path(event_id)
            .is_none()
            .then(|| demo_failure(event_id))
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
        sticker_asset_path(mxc_asset(mxc))
    }

    fn sticker_failed(&self, mxc: &str) -> bool {
        demo_failure(mxc_asset(mxc)).is_some() && self.sticker_path(mxc).is_none()
    }
}

pub fn assets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo")
}

pub fn user_avatar_path() -> Option<PathBuf> {
    asset(&format!("avatar-{}.png", localpart(data::own_user())))
}

fn probe(prefix: &str, name: &str) -> Option<PathBuf> {
    asset(&format!("{prefix}-{name}.gif"))
        .or_else(|| asset(&format!("{prefix}-{name}.webp")))
        .or_else(|| asset(&format!("{prefix}-{name}.png")))
}

fn sticker_asset_path(asset: &str) -> Option<PathBuf> {
    probe("sticker", asset).or_else(|| probe("thumbnail", asset))
}

fn asset(name: &str) -> Option<PathBuf> {
    let path = assets_dir().join(name);
    path.is_file().then_some(path)
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
