use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;

use super::{animation, cache};

const DECODE_LANE_CAP: usize = 1024;
const MAX_DECODE_WORKERS: usize = 3;

thread_local! {
    static DECODE_QUEUE: RefCell<Option<Arc<DecodeQueue>>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy)]
pub(super) enum Lane {
    Avatar,
    Static,
    Animation,
}

struct Job {
    path: PathBuf,
    epoch: u64,
}

struct QueueInner {
    avatar: VecDeque<Job>,
    static_img: VecDeque<Job>,
    animation: VecDeque<Job>,
}

impl QueueInner {
    fn new() -> Self {
        Self {
            avatar: VecDeque::new(),
            static_img: VecDeque::new(),
            animation: VecDeque::new(),
        }
    }

    fn lane_mut(&mut self, lane: Lane) -> &mut VecDeque<Job> {
        match lane {
            Lane::Avatar => &mut self.avatar,
            Lane::Static => &mut self.static_img,
            Lane::Animation => &mut self.animation,
        }
    }

    fn take_front(&mut self) -> Option<(Lane, Job)> {
        for lane in [Lane::Avatar, Lane::Static, Lane::Animation] {
            if let Some(job) = self.lane_mut(lane).pop_front() {
                return Some((lane, job));
            }
        }
        None
    }

    fn push_back_evicting_oldest(&mut self, lane: Lane, job: Job) -> Option<Job> {
        let queue = self.lane_mut(lane);
        queue.push_back(job);
        (queue.len() > DECODE_LANE_CAP)
            .then(|| queue.pop_front())
            .flatten()
    }

    fn clear(&mut self) {
        self.avatar.clear();
        self.static_img.clear();
        self.animation.clear();
    }
}

struct DecodeQueue {
    inner: Mutex<QueueInner>,
    signal: Condvar,
}

fn lock(mutex: &Mutex<QueueInner>) -> MutexGuard<'_, QueueInner> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn ensure_workers() {
    DECODE_QUEUE.with_borrow_mut(|slot| {
        if slot.is_some() {
            return;
        }
        let queue = Arc::new(DecodeQueue {
            inner: Mutex::new(QueueInner::new()),
            signal: Condvar::new(),
        });
        let worker_count = thread::available_parallelism()
            .map_or(2, |n| n.get().saturating_sub(1))
            .clamp(1, MAX_DECODE_WORKERS);
        let mut spawned = 0;
        for index in 0..worker_count {
            let queue = Arc::clone(&queue);
            match thread::Builder::new()
                .name(format!("u2dm-image-decode-{index}"))
                .spawn(move || decode_worker(&queue))
            {
                Ok(_) => spawned += 1,
                Err(e) => tracing::warn!("failed to spawn image decode thread: {e}"),
            }
        }
        if spawned > 0 {
            *slot = Some(queue);
        }
    });
}

fn decode_worker(queue: &Arc<DecodeQueue>) {
    loop {
        let (lane, job) = next_job(queue);
        run_job(lane, job);
    }
}

fn next_job(queue: &Arc<DecodeQueue>) -> (Lane, Job) {
    let mut inner = lock(&queue.inner);
    loop {
        if let Some(picked) = inner.take_front() {
            return picked;
        }
        inner = queue
            .signal
            .wait(inner)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

fn run_job(lane: Lane, job: Job) {
    let Job { path, epoch } = job;
    match lane {
        Lane::Avatar | Lane::Static => {
            let decoded = cache::decode_rgba(&path);
            drop(slint::invoke_from_event_loop(move || {
                cache::on_decoded(&path, decoded, epoch);
            }));
        }
        Lane::Animation => {
            let decoded = animation::decode_raw(&path);
            drop(slint::invoke_from_event_loop(move || {
                animation::on_decoded(&path, decoded, epoch);
            }));
        }
    }
}

pub(super) type Evicted = Option<PathBuf>;

pub(super) fn submit(lane: Lane, path: PathBuf) -> Evicted {
    ensure_workers();
    let epoch = super::current_epoch();
    DECODE_QUEUE.with_borrow(|slot| {
        let queue = slot.as_ref()?;
        let evicted = {
            let mut inner = lock(&queue.inner);
            inner.push_back_evicting_oldest(lane, Job { path, epoch })
        };
        queue.signal.notify_all();
        evicted.map(|job| job.path)
    })
}

pub(super) fn clear() {
    DECODE_QUEUE.with_borrow(|slot| {
        if let Some(queue) = slot.as_ref() {
            lock(&queue.inner).clear();
        }
    });
}
