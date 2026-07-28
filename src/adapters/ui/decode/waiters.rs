use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::Image;

use super::workers::{self, Lane};

type ImageReadyFn = Rc<dyn Fn(&str, DecodeOutcome<'_>)>;
type AvatarReadyFn = Rc<dyn Fn(&[AvatarSlot], DecodeOutcome<'_>)>;

thread_local! {
    static WAITERS: RefCell<Waiters> = RefCell::new(Waiters::default());
    static IMAGE_READY_FN: RefCell<Option<ImageReadyFn>> = const { RefCell::new(None) };
    static AVATAR_READY_FN: RefCell<Option<AvatarReadyFn>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy)]
pub enum DecodeOutcome<'a> {
    Ready(&'a Image),
    Failed,
    Deferred,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum AvatarSlot {
    Message(String),
    Room(String),
    Space(String),
    User,
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
struct Waiters {
    media: HashMap<PathBuf, Vec<String>>,
    avatars: HashMap<PathBuf, Vec<AvatarSlot>>,
}

impl Waiters {
    fn join_media(&mut self, path: &Path, unique_id: &str) -> Registration {
        let is_first = !self.media.contains_key(path);
        let waiting = self.media.entry(path.to_path_buf()).or_default();
        if !waiting.iter().any(|id| id == unique_id) {
            waiting.push(unique_id.to_owned());
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
            unique_ids: self.media.remove(path).unwrap_or_default(),
            slots: Vec::new(),
        }
    }

    fn take_avatars(&mut self, path: &Path) -> Drained {
        Drained {
            unique_ids: Vec::new(),
            slots: self.avatars.remove(path).unwrap_or_default(),
        }
    }

    fn take_all(&mut self, path: &Path) -> Drained {
        Drained {
            unique_ids: self.media.remove(path).unwrap_or_default(),
            slots: self.avatars.remove(path).unwrap_or_default(),
        }
    }
}

pub(super) struct Drained {
    unique_ids: Vec<String>,
    slots: Vec<AvatarSlot>,
}

impl Drained {
    pub(super) fn unique_ids(&self) -> &[String] {
        &self.unique_ids
    }

    pub(super) fn notify(&self, outcome: DecodeOutcome<'_>) {
        notify_media(&self.unique_ids, outcome);
        notify_avatars(&self.slots, outcome);
    }
}

pub(super) fn notify_media(unique_ids: &[String], outcome: DecodeOutcome<'_>) {
    if unique_ids.is_empty() {
        return;
    }
    let Some(ready) = IMAGE_READY_FN.with_borrow(Clone::clone) else {
        return;
    };
    for unique_id in unique_ids {
        ready(unique_id, outcome);
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

pub(super) fn enqueue_media(path: &Path, unique_id: &str, lane: Lane) {
    if WAITERS
        .with_borrow_mut(|waiters| waiters.join_media(path, unique_id))
        .starts_decode()
    {
        start_decode(lane, path.to_path_buf());
    }
}

pub(super) fn enqueue_avatar(path: &Path, slot: AvatarSlot) {
    if WAITERS
        .with_borrow_mut(|waiters| waiters.join_avatar(path, slot))
        .starts_decode()
    {
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
    WAITERS
        .with_borrow_mut(|waiters| match lane {
            Lane::Avatar => waiters.take_avatars(&evicted),
            Lane::Static | Lane::Animation => waiters.take_media(&evicted),
        })
        .notify(DecodeOutcome::Deferred);
}

pub(super) fn deliver(path: &Path, outcome: DecodeOutcome<'_>) {
    WAITERS
        .with_borrow_mut(|waiters| waiters.take_all(path))
        .notify(outcome);
}

pub(super) fn take_media(path: &Path) -> Drained {
    WAITERS.with_borrow_mut(|waiters| waiters.take_media(path))
}

pub(super) fn clear() {
    WAITERS.with_borrow_mut(|waiters| *waiters = Waiters::default());
}

pub fn set_image_ready(ready: impl Fn(&str, DecodeOutcome<'_>) + 'static) {
    IMAGE_READY_FN.with_borrow_mut(|slot| *slot = Some(Rc::new(ready)));
}

pub fn set_avatar_ready(ready: impl Fn(&[AvatarSlot], DecodeOutcome<'_>) + 'static) {
    AVATAR_READY_FN.with_borrow_mut(|slot| *slot = Some(Rc::new(ready)));
}
