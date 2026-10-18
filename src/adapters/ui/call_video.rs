use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use slint::{ComponentHandle, Image, Model, Rgba8Pixel, SharedPixelBuffer};

use super::backend::UiBackend;
use super::fields::CallTileFields;
use crate::domain::call::video::{FeedId, FrameSize, Framing};
use crate::ports::call::{CallVideoPort, FrameSink, VideoFrame};

const FIRST_SIZE: FrameSize = FrameSize::new(320, 180);

thread_local! {
    static PORT: RefCell<Option<Arc<dyn CallVideoPort>>> = const { RefCell::new(None) };
    static SHOWN: RefCell<HashMap<FeedId, Shown>> = RefCell::new(HashMap::new());
}

static NEXT_ATTACHMENT: AtomicU64 = AtomicU64::new(1);

struct Shown {
    attachment: u64,
    wanted: FrameSize,
    frame: Option<Image>,
    frames: u64,
}

enum Change {
    Attach(FeedId, Framing, u64),
    Detach(FeedId),
}

pub fn install(port: Option<Arc<dyn CallVideoPort>>) {
    PORT.with(|cell| *cell.borrow_mut() = port);
}

fn port() -> Option<Arc<dyn CallVideoPort>> {
    PORT.with(|cell| cell.borrow().clone())
}

pub fn follow<B: UiBackend>(window: &B::Window, feeds: &[(FeedId, Framing)]) {
    let changes = SHOWN.with(|cell| {
        let mut shown = cell.borrow_mut();
        let mut changes: Vec<Change> = shown
            .keys()
            .filter(|feed| !feeds.iter().any(|(wanted, _)| wanted == *feed))
            .cloned()
            .map(Change::Detach)
            .collect();
        for change in &changes {
            if let Change::Detach(feed) = change {
                shown.remove(feed);
            }
        }
        for (feed, framing) in feeds {
            if shown.contains_key(feed) {
                continue;
            }
            let attachment = NEXT_ATTACHMENT.fetch_add(1, Ordering::Relaxed);
            shown.insert(
                feed.clone(),
                Shown {
                    attachment,
                    wanted: FIRST_SIZE,
                    frame: None,
                    frames: 0,
                },
            );
            changes.push(Change::Attach(feed.clone(), *framing, attachment));
        }
        changes
    });
    let Some(port) = port() else {
        return;
    };
    for change in changes {
        match change {
            Change::Attach(feed, framing, attachment) => {
                tracing::debug!(%feed, ?framing, "showing a call video");
                let sink = sink::<B>(window.as_weak(), feed.clone(), attachment);
                port.attach(&feed, FIRST_SIZE, framing, sink);
            }
            Change::Detach(feed) => {
                tracing::debug!(%feed, "no tile shows this call video any more");
                port.detach(&feed);
            }
        }
    }
}

pub fn sized(feed: &str, width: i32, height: i32) {
    let feed = FeedId::new(feed);
    let wanted = FrameSize::new(
        u32::try_from(width).unwrap_or(0).max(1),
        u32::try_from(height).unwrap_or(0).max(1),
    );
    let resized = SHOWN.with(|cell| match cell.borrow_mut().get_mut(&feed) {
        Some(entry) if entry.wanted != wanted => {
            entry.wanted = wanted;
            true
        }
        _ => false,
    });
    if resized && let Some(port) = port() {
        port.resize(&feed, wanted);
    }
}

pub fn fill<B: UiBackend>(tiles: &mut [B::CallTile]) {
    SHOWN.with(|cell| {
        let shown = cell.borrow();
        for tile in tiles {
            let feed = FeedId::new(tile.feed());
            if let Some(frame) = shown.get(&feed).and_then(|entry| entry.frame.clone()) {
                tile.set_frame(frame);
                tile.set_has_frame(true);
            }
        }
    });
}

#[cfg(feature = "demo")]
pub fn frames_of(feed: &str) -> u64 {
    SHOWN.with(|cell| {
        cell.borrow()
            .get(&FeedId::new(feed))
            .map_or(0, |entry| entry.frames)
    })
}

fn sink<B: UiBackend>(weak: slint::Weak<B::Window>, feed: FeedId, attachment: u64) -> FrameSink {
    let in_flight = Arc::new(AtomicBool::new(false));
    Arc::new(move |frame: &dyn VideoFrame| {
        if in_flight.swap(true, Ordering::AcqRel) {
            return;
        }
        let size = frame.size();
        let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(size.width, size.height);
        if !frame.write_rgba(pixels.make_mut_bytes()) {
            in_flight.store(false, Ordering::Release);
            return;
        }
        let landed = Arc::clone(&in_flight);
        let feed = feed.clone();
        let posted = weak.upgrade_in_event_loop(move |_| {
            landed.store(false, Ordering::Release);
            show::<B>(&feed, attachment, pixels);
        });
        if posted.is_err() {
            in_flight.store(false, Ordering::Release);
        }
    })
}

fn show<B: UiBackend>(feed: &FeedId, attachment: u64, pixels: SharedPixelBuffer<Rgba8Pixel>) {
    let image = Image::from_rgba8(pixels);
    let current = SHOWN.with(|cell| match cell.borrow_mut().get_mut(feed) {
        Some(entry) if entry.attachment == attachment => {
            entry.frame = Some(image.clone());
            entry.frames = entry.frames.saturating_add(1);
            true
        }
        _ => false,
    });
    if !current {
        return;
    }
    let models = B::models();
    let tiles = &models.call_tiles;
    for row in 0..tiles.row_count() {
        let Some(mut tile) = tiles.row_data(row) else {
            continue;
        };
        if tile.feed() != feed.as_ref() {
            continue;
        }
        tile.set_frame(image.clone());
        tile.set_has_frame(true);
        tiles.set_row_data(row, tile);
    }
}
