use std::path::{Path, PathBuf};

use super::data;
use crate::ports::media::MediaCache;

pub struct DemoMediaCache;

impl MediaCache for DemoMediaCache {
    fn thumbnail_path(&self, event_id: &str) -> Option<PathBuf> {
        asset(&format!("thumbnail-{event_id}.png"))
    }

    fn avatar_path(&self, sender: &str) -> Option<PathBuf> {
        asset(&format!("avatar-{}.png", localpart(sender)))
    }

    fn space_avatar_path(&self, mxc: &str) -> Option<PathBuf> {
        asset(&format!("space-{mxc}.png"))
    }
}

pub fn assets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo")
}

pub fn user_avatar_path() -> Option<PathBuf> {
    asset(&format!("avatar-{}.png", localpart(data::own_user())))
}

fn asset(name: &str) -> Option<PathBuf> {
    let path = assets_dir().join(name);
    path.is_file().then_some(path)
}

fn localpart(user_id: &str) -> &str {
    user_id
        .trim_start_matches('@')
        .split(':')
        .next()
        .unwrap_or(user_id)
}
