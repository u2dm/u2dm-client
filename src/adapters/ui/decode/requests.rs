use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::{mem, slice};

use super::cache::Decoded;
use super::waiters::{AvatarSlot, DecodeOutcome};
use super::{animation, cache, waiters};

thread_local! {
    static NEEDS: RefCell<Needs> = RefCell::new(Needs::default());
    static PENDING: RefCell<Vec<Request>> = const { RefCell::new(Vec::new()) };
}

#[derive(PartialEq, Eq)]
enum Request {
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
struct Needs {
    media: HashMap<String, MediaNeed>,
    avatars: HashMap<AvatarSlot, PathBuf>,
    stickers: HashMap<String, PathBuf>,
}

pub fn record_media_need(unique_id: &str, thumbnail: Option<PathBuf>, avatar: Option<PathBuf>) {
    if thumbnail.is_none() && avatar.is_none() {
        NEEDS.with_borrow_mut(|needs| needs.media.remove(unique_id));
        return;
    }
    NEEDS.with_borrow_mut(|needs| {
        needs
            .media
            .insert(unique_id.to_owned(), MediaNeed { thumbnail, avatar });
    });
}

pub fn record_avatar_need(slot: AvatarSlot, path: PathBuf) {
    NEEDS.with_borrow_mut(|needs| needs.avatars.insert(slot, path));
}

pub fn record_sticker_need(key: &str, path: PathBuf) {
    NEEDS.with_borrow_mut(|needs| needs.stickers.insert(key.to_owned(), path));
}

pub fn forget_all_media_needs() {
    NEEDS.with_borrow_mut(|needs| needs.media.clear());
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
    let armed = PENDING.with_borrow_mut(|pending| {
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
    for request in PENDING.with_borrow_mut(mem::take) {
        match request {
            Request::Media(unique_id) => resolve_media(&unique_id),
            Request::Avatar(slot) => resolve_avatar(&slot),
            Request::Sticker(key) => resolve_sticker(&key),
        }
    }
}

fn resolve_sticker(key: &str) {
    let Some(path) = NEEDS.with_borrow(|needs| needs.stickers.get(key).cloned()) else {
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
    let Some(path) = NEEDS.with_borrow(|needs| needs.avatars.get(slot).cloned()) else {
        return;
    };
    if let Some(image) = cache::load_avatar_async(&path, slot.clone()) {
        waiters::notify_avatars(slice::from_ref(slot), DecodeOutcome::Ready(&image));
    }
}

fn resolve_media(unique_id: &str) {
    let Some(need) = NEEDS.with_borrow(|needs| needs.media.get(unique_id).cloned()) else {
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

pub(super) fn clear() {
    PENDING.with_borrow_mut(Vec::clear);
    NEEDS.with_borrow_mut(|needs| *needs = Needs::default());
}
