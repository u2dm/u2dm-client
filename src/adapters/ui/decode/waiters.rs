use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::Image;

use super::cache::DecodeFailure;
use super::requests::Needs;
use super::slots::{AvatarSlot, MediaSlot};
use super::with_media;
use super::workers::{self, Lane};

type ImageReadyFn = Rc<dyn Fn(&MediaSlot, DecodeOutcome<'_>)>;
type AvatarReadyFn = Rc<dyn Fn(&[AvatarSlot], DecodeOutcome<'_>)>;

thread_local! {
    static IMAGE_READY_FN: RefCell<Option<ImageReadyFn>> = const { RefCell::new(None) };
    static AVATAR_READY_FN: RefCell<Option<AvatarReadyFn>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy)]
pub enum DecodeOutcome<'a> {
    Ready(&'a Image),
    Failed(DecodeFailure),
    Deferred,
}

enum Registration {
    FirstWaiter,
    JoinedExisting,
}

impl Registration {
    fn starts_decode(&self) -> bool {
        matches!(self, Self::FirstWaiter)
    }
}

#[derive(Default)]
pub(super) struct Waiters {
    media: HashMap<PathBuf, Vec<MediaSlot>>,
    avatars: HashMap<PathBuf, Vec<AvatarSlot>>,
}

impl Waiters {
    fn join_media(&mut self, path: &Path, slot: &MediaSlot) -> Registration {
        let is_first = !self.media.contains_key(path);
        let waiting = self.media.entry(path.to_path_buf()).or_default();
        if !waiting.contains(slot) {
            waiting.push(slot.clone());
        }
        if is_first {
            Registration::FirstWaiter
        } else {
            Registration::JoinedExisting
        }
    }

    fn join_avatar(&mut self, path: &Path, slot: AvatarSlot) -> Registration {
        let is_first = !self.avatars.contains_key(path);
        let waiting = self.avatars.entry(path.to_path_buf()).or_default();
        if !waiting.contains(&slot) {
            waiting.push(slot);
        }
        if is_first {
            Registration::FirstWaiter
        } else {
            Registration::JoinedExisting
        }
    }

    fn take_media(&mut self, path: &Path) -> Drained {
        Drained {
            media: self.media.remove(path).unwrap_or_default(),
            avatars: Vec::new(),
        }
    }

    fn take_avatars(&mut self, path: &Path) -> Drained {
        Drained {
            media: Vec::new(),
            avatars: self.avatars.remove(path).unwrap_or_default(),
        }
    }

    fn take_all(&mut self, path: &Path) -> Drained {
        Drained {
            media: self.media.remove(path).unwrap_or_default(),
            avatars: self.avatars.remove(path).unwrap_or_default(),
        }
    }
}

fn with_waiters<R>(f: impl FnOnce(&mut Waiters) -> R) -> R {
    with_media(|media| f(&mut media.waiters))
}

fn drain_expected(path: &Path, take: impl FnOnce(&mut Waiters, &Path) -> Drained) -> Drained {
    with_media(|media| {
        let mut drained = take(&mut media.waiters, path);
        drained.keep_expected(&media.needs, path);
        drained
    })
}

pub(super) struct Drained {
    media: Vec<MediaSlot>,
    avatars: Vec<AvatarSlot>,
}

impl Drained {
    fn keep_expected(&mut self, needs: &Needs, path: &Path) {
        self.media.retain(|slot| needs.expects_media(slot, path));
        self.avatars.retain(|slot| needs.expects_avatar(slot, path));
    }

    pub(super) fn media_slots(&self) -> &[MediaSlot] {
        &self.media
    }

    pub(super) fn notify(&self, outcome: DecodeOutcome<'_>) {
        notify_media(&self.media, outcome);
        notify_avatars(&self.avatars, outcome);
    }
}

pub(super) fn notify_media(slots: &[MediaSlot], outcome: DecodeOutcome<'_>) {
    if slots.is_empty() {
        return;
    }
    let Some(ready) = IMAGE_READY_FN.with_borrow(Clone::clone) else {
        return;
    };
    for slot in slots {
        ready(slot, outcome);
    }
}

pub(super) fn notify_avatars(slots: &[AvatarSlot], outcome: DecodeOutcome<'_>) {
    if slots.is_empty() {
        return;
    }
    if let Some(ready) = AVATAR_READY_FN.with_borrow(Clone::clone) {
        ready(slots, outcome);
    }
}

pub(super) fn enqueue_media(path: &Path, slot: &MediaSlot, lane: Lane) {
    if with_waiters(|waiters| waiters.join_media(path, slot)).starts_decode() {
        start_decode(lane, path.to_path_buf());
    }
}

pub(super) fn enqueue_avatar(path: &Path, slot: AvatarSlot) {
    if with_waiters(|waiters| waiters.join_avatar(path, slot)).starts_decode() {
        start_decode(Lane::Avatar, path.to_path_buf());
    }
}

fn start_decode(lane: Lane, path: PathBuf) {
    let Some(evicted) = workers::submit(lane, path) else {
        return;
    };
    tracing::warn!(
        "decode lane at capacity, deferred {}; it will be re-requested",
        evicted.display()
    );
    drain_expected(&evicted, |waiters, path| match lane {
        Lane::Avatar => waiters.take_avatars(path),
        Lane::Static | Lane::Animation => waiters.take_media(path),
    })
    .notify(DecodeOutcome::Deferred);
}

pub(super) fn deliver(path: &Path, outcome: DecodeOutcome<'_>) {
    drain_expected(path, Waiters::take_all).notify(outcome);
}

pub(super) fn take_media(path: &Path) -> Drained {
    drain_expected(path, Waiters::take_media)
}

pub fn set_image_ready(ready: impl Fn(&MediaSlot, DecodeOutcome<'_>) + 'static) {
    IMAGE_READY_FN.with_borrow_mut(|slot| *slot = Some(Rc::new(ready)));
}

pub fn set_avatar_ready(ready: impl Fn(&[AvatarSlot], DecodeOutcome<'_>) + 'static) {
    AVATAR_READY_FN.with_borrow_mut(|slot| *slot = Some(Rc::new(ready)));
}
