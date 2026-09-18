use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::{mem, slice};

use super::cache::Decoded;
use super::waiters::{AvatarSlot, DecodeOutcome};
use super::{animation, cache, waiters, with_media};

#[derive(PartialEq, Eq)]
pub(super) enum Request {
    Media(String),
    Avatar(AvatarSlot),
    Sticker(String),
}

#[derive(Clone)]
struct MediaNeed {
    thumbnail: Option<PathBuf>,
    avatar: Option<PathBuf>,
}

#[derive(Default)]
pub(super) struct Needs {
    media: HashMap<String, MediaNeed>,
    avatars: HashMap<AvatarSlot, PathBuf>,
    stickers: HashMap<String, PathBuf>,
    stickers_asked_before_download: HashSet<String>,
}

pub fn record_media_need(unique_id: &str, thumbnail: Option<PathBuf>, avatar: Option<PathBuf>) {
    if thumbnail.is_none() && avatar.is_none() {
        with_media(|media| media.needs.media.remove(unique_id));
        return;
    }
    with_media(|media| {
        media
            .needs
            .media
            .insert(unique_id.to_owned(), MediaNeed { thumbnail, avatar });
    });
}

pub fn record_avatar_need(slot: AvatarSlot, path: PathBuf) {
    with_media(|media| media.needs.avatars.insert(slot, path));
}

pub fn record_sticker_need(key: &str, path: PathBuf) {
    let already_asked = with_media(|media| {
        let needs = &mut media.needs;
        needs.stickers.insert(key.to_owned(), path);
        needs.stickers_asked_before_download.remove(key)
    });
    if already_asked {
        request_sticker(key);
    }
}

pub fn forget_all_media_needs() {
    with_media(|media| media.needs.media.clear());
}

pub fn request_avatar(slot: &AvatarSlot) {
    queue(Request::Avatar(slot.clone()));
}

pub fn request_media(unique_id: &str) {
    queue(Request::Media(unique_id.to_owned()));
}

pub fn request_sticker(key: &str) {
    queue(Request::Sticker(key.to_owned()));
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
            Request::Media(unique_id) => resolve_media(&unique_id),
            Request::Avatar(slot) => resolve_avatar(&slot),
            Request::Sticker(key) => resolve_sticker(&key),
        }
    }
}

fn resolve_sticker(key: &str) {
    let path = with_media(|media| {
        let needs = &mut media.needs;
        let path = needs.stickers.get(key).cloned();
        if path.is_none() {
            needs.stickers_asked_before_download.insert(key.to_owned());
        }
        path
    });
    let Some(path) = path else {
        return;
    };
    announce(key, &animation::load_thumbnail(&path, key));
}

fn announce(unique_id: &str, decoded: &Decoded) {
    let outcome = match decoded {
        Decoded::Ready(image) => DecodeOutcome::Ready(image),
        Decoded::Failed => DecodeOutcome::Failed,
        Decoded::Pending => return,
    };
    waiters::notify_media(&[unique_id.to_owned()], outcome);
}

fn resolve_avatar(slot: &AvatarSlot) {
    let Some(path) = with_media(|media| media.needs.avatars.get(slot).cloned()) else {
        return;
    };
    if let Some(image) = cache::load_avatar_async(&path, slot.clone()) {
        waiters::notify_avatars(slice::from_ref(slot), DecodeOutcome::Ready(&image));
    }
}

fn resolve_media(unique_id: &str) {
    let Some(need) = with_media(|media| media.needs.media.get(unique_id).cloned()) else {
        return;
    };
    if let Some(thumbnail) = &need.thumbnail {
        announce(unique_id, &animation::load_thumbnail(thumbnail, unique_id));
    }
    if let Some(avatar) = &need.avatar {
        let slot = AvatarSlot::Message(unique_id.to_owned());
        if let Some(image) = cache::load_avatar_async(avatar, slot.clone()) {
            waiters::notify_avatars(slice::from_ref(&slot), DecodeOutcome::Ready(&image));
        }
    }
}
