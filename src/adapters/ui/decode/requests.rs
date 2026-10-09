use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::{mem, slice};

use super::cache::Decoded;
use super::slots::{AvatarSlot, MediaSlot, TimelineItemKey};
use super::waiters::DecodeOutcome;
use super::{animation, cache, waiters, with_media};

#[derive(PartialEq, Eq)]
pub(super) enum Request {
    Media(TimelineItemKey),
    Avatar(AvatarSlot),
    Sticker(String),
    Reaction(String),
}

pub(super) enum PreviewPick {
    AlreadyShown,
    New,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ShownAvatar {
    StillExpected,
    Outdated,
}

#[derive(Default)]
pub(super) struct Needs {
    media: HashMap<MediaSlot, PathBuf>,
    avatars: HashMap<AvatarSlot, PathBuf>,
    avatars_asked: HashSet<AvatarSlot>,
    stickers_asked_before_download: HashSet<String>,
    rows_showing_reaction: HashMap<String, HashSet<String>>,
}

impl Needs {
    pub(super) fn expect_media(&mut self, slot: &MediaSlot, path: Option<&Path>) {
        match path {
            Some(path) => {
                self.media.insert(slot.clone(), path.to_path_buf());
            }
            None => {
                self.media.remove(slot);
            }
        }
    }

    pub(super) fn expect_avatar(&mut self, slot: &AvatarSlot, path: Option<&Path>) {
        match path {
            Some(path) => {
                self.avatars.insert(slot.clone(), path.to_path_buf());
            }
            None => {
                self.avatars.remove(slot);
            }
        }
    }

    pub(super) fn expects_media(&self, slot: &MediaSlot, path: &Path) -> bool {
        self.media
            .get(slot)
            .is_some_and(|expected| expected == path)
    }

    pub(super) fn expects_avatar(&self, slot: &AvatarSlot, path: &Path) -> bool {
        self.avatars
            .get(slot)
            .is_some_and(|expected| expected == path)
    }

    pub(super) fn adopt_preview(&mut self, pick: &AvatarSlot) -> PreviewPick {
        let shown = self.avatars.contains_key(pick);
        self.avatars
            .retain(|slot, _| !slot.is_attachment_preview() || slot == pick);
        if shown {
            PreviewPick::AlreadyShown
        } else {
            PreviewPick::New
        }
    }

    fn forget_timeline(&mut self) {
        self.media.retain(|slot, _| !slot.belongs_to_timeline());
        self.avatars.retain(|slot, _| !slot.belongs_to_timeline());
        self.rows_showing_reaction.clear();
    }
}

pub fn record_media_need(item: &TimelineItemKey, thumbnail: Option<&Path>, avatar: Option<&Path>) {
    let thumbnail_slot = MediaSlot::Thumbnail(item.clone());
    let avatar_slot = AvatarSlot::Message(item.clone());
    with_media(|media| {
        media.needs.expect_media(&thumbnail_slot, thumbnail);
        media.needs.expect_avatar(&avatar_slot, avatar);
    });
}

pub fn record_avatar_need(slot: &AvatarSlot, path: Option<&Path>) -> ShownAvatar {
    let (shown, asked) = with_media(|media| {
        let needs = &mut media.needs;
        let shown = match path {
            Some(path) if needs.expects_avatar(slot, path) => ShownAvatar::StillExpected,
            _ => ShownAvatar::Outdated,
        };
        needs.expect_avatar(slot, path);
        (shown, needs.avatars_asked.contains(slot))
    });
    if shown == ShownAvatar::Outdated && asked && path.is_some() {
        request_avatar(slot);
    }
    shown
}

pub fn record_sticker_need(key: &str, path: Option<&Path>) {
    let slot = MediaSlot::StickerCell(key.to_owned());
    let already_asked = with_media(|media| {
        let needs = &mut media.needs;
        needs.expect_media(&slot, path);
        path.is_some() && needs.stickers_asked_before_download.remove(key)
    });
    if already_asked {
        request_sticker(key);
    }
}

pub fn record_reaction_need(item: &TimelineItemKey, key: &str, path: &Path) {
    let slot = MediaSlot::Reaction(key.to_owned());
    with_media(|media| {
        let needs = &mut media.needs;
        needs.expect_media(&slot, Some(path));
        needs
            .rows_showing_reaction
            .entry(key.to_owned())
            .or_default()
            .insert(item.unique_id().to_owned());
    });
}

pub fn rows_showing_reaction(key: &str) -> Vec<String> {
    with_media(|media| {
        media
            .needs
            .rows_showing_reaction
            .get(key)
            .map(|rows| rows.iter().cloned().collect())
            .unwrap_or_default()
    })
}

pub fn forget_all_media_needs() {
    with_media(|media| media.needs.forget_timeline());
}

pub fn request_avatar(slot: &AvatarSlot) {
    queue(Request::Avatar(slot.clone()));
}

pub fn request_media(unique_id: &str) {
    queue(Request::Media(TimelineItemKey::current(unique_id)));
}

pub fn request_sticker(key: &str) {
    queue(Request::Sticker(key.to_owned()));
}

pub fn request_reaction(key: &str) {
    queue(Request::Reaction(key.to_owned()));
}

fn queue(request: Request) {
    let armed = with_media(|media| {
        let pending = &mut media.pending;
        if pending.contains(&request) {
            return false;
        }
        let was_empty = pending.is_empty();
        pending.push(request);
        was_empty
    });
    if armed && slint::invoke_from_event_loop(flush).is_err() {
        flush();
    }
}

fn flush() {
    for request in with_media(|media| mem::take(&mut media.pending)) {
        match request {
            Request::Media(item) => resolve_media(&item),
            Request::Avatar(slot) => resolve_avatar(&slot),
            Request::Sticker(key) => resolve_sticker(&key),
            Request::Reaction(key) => resolve_reaction(&key),
        }
    }
}

fn resolve_sticker(key: &str) {
    let slot = MediaSlot::StickerCell(key.to_owned());
    let path = with_media(|media| {
        let needs = &mut media.needs;
        let path = needs.media.get(&slot).cloned();
        if path.is_none() {
            needs.stickers_asked_before_download.insert(key.to_owned());
        }
        path
    });
    let Some(path) = path else {
        return;
    };
    announce(&slot, &animation::load_thumbnail(&path, &slot));
}

fn resolve_reaction(key: &str) {
    let slot = MediaSlot::Reaction(key.to_owned());
    let Some(path) = with_media(|media| media.needs.media.get(&slot).cloned()) else {
        return;
    };
    announce(&slot, &animation::load_thumbnail(&path, &slot));
}

fn announce(slot: &MediaSlot, decoded: &Decoded) {
    let outcome = match decoded {
        Decoded::Ready(image) => DecodeOutcome::Ready(image),
        Decoded::Failed(failure) => DecodeOutcome::Failed(*failure),
        Decoded::Pending => return,
    };
    waiters::notify_media(slice::from_ref(slot), outcome);
}

fn resolve_avatar(slot: &AvatarSlot) {
    let path = with_media(|media| {
        let needs = &mut media.needs;
        needs.avatars_asked.insert(slot.clone());
        needs.avatars.get(slot).cloned()
    });
    let Some(path) = path else {
        return;
    };
    show_resolved_avatar(&path, slot);
}

fn show_resolved_avatar(path: &Path, slot: &AvatarSlot) {
    if let Some(image) = cache::load_avatar_async(Some(path), slot.clone()) {
        waiters::notify_avatars(slice::from_ref(slot), DecodeOutcome::Ready(&image));
    }
}

fn resolve_media(item: &TimelineItemKey) {
    let thumbnail_slot = MediaSlot::Thumbnail(item.clone());
    let avatar_slot = AvatarSlot::Message(item.clone());
    let (thumbnail, avatar) = with_media(|media| {
        (
            media.needs.media.get(&thumbnail_slot).cloned(),
            media.needs.avatars.get(&avatar_slot).cloned(),
        )
    });
    if let Some(thumbnail) = &thumbnail {
        announce(
            &thumbnail_slot,
            &animation::load_thumbnail(thumbnail, &thumbnail_slot),
        );
    }
    if let Some(avatar) = &avatar {
        show_resolved_avatar(avatar, &avatar_slot);
    }
}
