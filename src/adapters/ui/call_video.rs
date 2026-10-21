use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use slint::{ComponentHandle, Image, Model, Rgba8Pixel, SharedPixelBuffer, VecModel};

use super::backend::UiBackend;
use super::fields::CallTileFields;
use crate::domain::call::video::{FeedId, FrameSize, Framing};
use crate::ports::call::{CallVideoPort, FrameSink, VideoFrame};

thread_local! {
    static PORT: RefCell<Option<Arc<dyn CallVideoPort>>> = const { RefCell::new(None) };
    static FEEDS: RefCell<Feeds> = RefCell::new(Feeds::default());
}

static NEXT_ATTACHMENT: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct Feeds {
    media_shown: bool,
    tiled: HashMap<FeedId, Framing>,
    seen: HashMap<FeedId, FrameSize>,
    shown: HashMap<FeedId, Shown>,
}

struct Shown {
    attachment: u64,
    wanted: FrameSize,
    frame: Option<Image>,
    frames: u64,
}

enum Change {
    Attach(FeedId, FrameSize, Framing, u64),
    Resize(FeedId, FrameSize),
    Detach(FeedId),
}

impl Feeds {
    fn settle(&mut self) -> Vec<Change> {
        let mut changes: Vec<Change> = self
            .shown
            .keys()
            .filter(|feed| self.wanted(feed).is_none())
            .cloned()
            .map(Change::Detach)
            .collect();
        for change in &changes {
            if let Change::Detach(feed) = change {
                self.shown.remove(feed);
            }
        }
        let wanted: Vec<(FeedId, FrameSize, Framing)> = self
            .tiled
            .keys()
            .filter_map(|feed| {
                let (size, framing) = self.wanted(feed)?;
                Some((feed.clone(), size, framing))
            })
            .collect();
        for (feed, size, framing) in wanted {
            match self.shown.get_mut(&feed) {
                Some(shown) if shown.wanted != size => {
                    shown.wanted = size;
                    changes.push(Change::Resize(feed, size));
                }
                Some(_) => {}
                None => {
                    let attachment = NEXT_ATTACHMENT.fetch_add(1, Ordering::Relaxed);
                    self.shown.insert(
                        feed.clone(),
                        Shown {
                            attachment,
                            wanted: size,
                            frame: None,
                            frames: 0,
                        },
                    );
                    changes.push(Change::Attach(feed, size, framing, attachment));
                }
            }
        }
        changes
    }

    fn wanted(&self, feed: &FeedId) -> Option<(FrameSize, Framing)> {
        if !self.media_shown {
            return None;
        }
        Some((*self.seen.get(feed)?, *self.tiled.get(feed)?))
    }
}

pub fn install(port: Option<Arc<dyn CallVideoPort>>) {
    PORT.with(|cell| *cell.borrow_mut() = port);
}

fn port() -> Option<Arc<dyn CallVideoPort>> {
    PORT.with(|cell| cell.borrow().clone())
}

pub fn follow<B: UiBackend>(window: &B::Window, feeds: &[(FeedId, Framing)]) {
    settle_with::<B>(window, |state| {
        state.tiled = feeds.iter().cloned().collect();
        state
            .seen
            .retain(|feed, _| feeds.iter().any(|(tiled, _)| tiled == feed));
    });
}

pub fn seen<B: UiBackend>(window: &B::Window, feed: &str, width: i32, height: i32, shown: bool) {
    let feed = FeedId::new(feed);
    let size = FrameSize::new(
        u32::try_from(width).unwrap_or(0).max(1),
        u32::try_from(height).unwrap_or(0).max(1),
    );
    settle_with::<B>(window, |state| {
        if shown {
            state.seen.insert(feed, size);
        } else {
            state.seen.remove(&feed);
        }
    });
}

pub fn media_shown<B: UiBackend>(window: &B::Window, on: bool) {
    tracing::debug!(on, "the call's videos are on screen");
    settle_with::<B>(window, |state| state.media_shown = on);
}

fn settle_with<B: UiBackend>(window: &B::Window, change: impl FnOnce(&mut Feeds)) {
    let changes = FEEDS.with(|cell| {
        let mut state = cell.borrow_mut();
        change(&mut state);
        state.settle()
    });
    let Some(port) = port() else {
        return;
    };
    for change in changes {
        match change {
            Change::Attach(feed, size, framing, attachment) => {
                tracing::debug!(%feed, ?framing, "showing a call video");
                let sink = sink::<B>(window.as_weak(), feed.clone(), attachment);
                port.attach(&feed, size, framing, sink);
            }
            Change::Resize(feed, size) => port.resize(&feed, size),
            Change::Detach(feed) => {
                tracing::debug!(%feed, "no tile on screen shows this call video");
                port.detach(&feed);
            }
        }
    }
}

pub fn last_frame(feed: &str) -> Option<Image> {
    FEEDS.with(|cell| {
        cell.borrow()
            .shown
            .get(&FeedId::new(feed))
            .and_then(|shown| shown.frame.clone())
    })
}

#[cfg(feature = "demo")]
pub fn frames_of(feed: &str) -> u64 {
    FEEDS.with(|cell| {
        cell.borrow()
            .shown
            .get(&FeedId::new(feed))
            .map_or(0, |entry| entry.frames)
    })
}

#[cfg(feature = "demo")]
pub fn attached() -> Vec<String> {
    let mut feeds: Vec<String> = FEEDS.with(|cell| {
        cell.borrow()
            .shown
            .keys()
            .map(|feed| feed.as_ref().to_owned())
            .collect()
    });
    feeds.sort();
    feeds
}

#[cfg(feature = "demo")]
pub fn media_is_shown() -> bool {
    FEEDS.with(|cell| cell.borrow().media_shown)
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
    let current = FEEDS.with(|cell| match cell.borrow_mut().shown.get_mut(feed) {
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
    show_in::<B>(&models.call_stage, feed, &image);
    show_in::<B>(&models.call_tiles, feed, &image);
}

fn show_in<B: UiBackend>(tiles: &VecModel<B::CallTile>, feed: &FeedId, image: &Image) {
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
