use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use slint::{ComponentHandle, Image, Model, Rgba8Pixel, SharedPixelBuffer, VecModel};

use super::backend::UiBackend;
use super::fields::CallTileFields;
use super::props::UiProps;
use crate::domain::call::video::{FeedId, FrameSize, Framing, LARGEST_SHOWN};
use crate::ports::call::{CallVideoPort, FrameSink, VideoFrame};

thread_local! {
    static PORT: RefCell<Option<Arc<dyn CallVideoPort>>> = const { RefCell::new(None) };
    static FEEDS: RefCell<Feeds> = RefCell::new(Feeds::default());
}

static NEXT_ATTACHMENT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Host {
    Main,
    CallWindow,
}

#[derive(Default)]
struct Feeds {
    media_shown: HashSet<Host>,
    tiled: HashMap<FeedId, Framing>,
    seen: HashMap<FeedId, HashMap<Host, FrameSize>>,
    preview: Preview,
    shown: HashMap<FeedId, Shown>,
}

#[derive(Default)]
struct Preview {
    feed: Option<FeedId>,
    seen: Option<(FeedId, FrameSize)>,
}

impl Preview {
    fn wanted(&self, feed: &FeedId) -> Option<FrameSize> {
        let (seen, size) = self.seen.as_ref()?;
        (self.feed.as_ref() == Some(feed) && seen == feed).then_some(*size)
    }
}

struct Shown {
    attachment: u64,
    wanted: FrameSize,
    frame: Option<Image>,
    source: FrameSize,
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
        let previewed = self
            .preview
            .feed
            .iter()
            .filter(|feed| !self.tiled.contains_key(*feed));
        let wanted: Vec<(FeedId, FrameSize, Framing)> = self
            .tiled
            .keys()
            .chain(previewed)
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
                            source: FrameSize::new(0, 0),
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
        match (self.on_a_tile(feed), self.preview.wanted(feed)) {
            (Some((tile, framing)), Some(preview)) => Some((larger(tile, preview), framing)),
            (Some(tile), None) => Some(tile),
            (None, Some(preview)) => Some((preview, Framing::Cover)),
            (None, None) => None,
        }
    }

    fn on_a_tile(&self, feed: &FeedId) -> Option<(FrameSize, Framing)> {
        let framing = *self.tiled.get(feed)?;
        let size = self
            .seen
            .get(feed)?
            .iter()
            .filter(|(host, _)| self.media_shown.contains(host))
            .map(|(_, size)| *size)
            .reduce(larger)?;
        Some((size, framing))
    }
}

fn larger(one: FrameSize, other: FrameSize) -> FrameSize {
    FrameSize::new(one.width.max(other.width), one.height.max(other.height))
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

fn asked(width: i32, height: i32) -> FrameSize {
    FrameSize::new(
        u32::try_from(width).unwrap_or(0).max(1),
        u32::try_from(height).unwrap_or(0).max(1),
    )
    .framed(LARGEST_SHOWN, Framing::Contain)
}

pub fn seen<B: UiBackend>(
    window: &B::Window,
    host: Host,
    feed: &str,
    width: i32,
    height: i32,
    shown: bool,
) {
    let feed = FeedId::new(feed);
    let size = asked(width, height);
    settle_with::<B>(window, |state| {
        if shown {
            state.seen.entry(feed).or_default().insert(host, size);
        } else if let Some(hosts) = state.seen.get_mut(&feed) {
            hosts.remove(&host);
        }
    });
}

pub fn preview_seen<B: UiBackend>(
    window: &B::Window,
    feed: &str,
    width: i32,
    height: i32,
    shown: bool,
) {
    let feed = FeedId::new(feed);
    let size = asked(width, height);
    settle_with::<B>(window, |state| {
        state.preview.seen = shown.then_some((feed, size));
    });
}

pub fn follow_preview<B: UiBackend>(window: &B::Window, feed: Option<&FeedId>) {
    tracing::debug!(feed = ?feed, "the call sheet previews a camera");
    settle_with::<B>(window, |state| state.preview.feed = feed.cloned());
    window.apply_call_preview(feed.and_then(|feed| last_frame(feed.as_ref())));
}

pub fn media_shown<B: UiBackend>(window: &B::Window, host: Host, on: bool) {
    tracing::debug!(?host, on, "the call's videos are on screen");
    settle_with::<B>(window, |state| {
        if on {
            state.media_shown.insert(host);
        } else {
            state.media_shown.remove(&host);
        }
    });
}

pub fn forget<B: UiBackend>(window: &B::Window, host: Host) {
    settle_with::<B>(window, |state| {
        state.media_shown.remove(&host);
        for hosts in state.seen.values_mut() {
            hosts.remove(&host);
        }
    });
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

pub fn show_last<B: UiBackend>(tile: &mut B::CallTile) {
    let last = FEEDS.with(|cell| {
        let state = cell.borrow();
        let shown = state.shown.get(&FeedId::new(tile.feed()))?;
        Some((shown.frame.clone()?, shown.source))
    });
    if let Some((frame, source)) = last {
        tile.set_frame(frame);
        tile.set_has_frame(true);
        put_source::<B>(tile, source);
    }
}

fn put_source<B: UiBackend>(tile: &mut B::CallTile, source: FrameSize) {
    tile.set_source_width(i32::try_from(source.width).unwrap_or(i32::MAX));
    tile.set_source_height(i32::try_from(source.height).unwrap_or(i32::MAX));
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
pub fn wanted_of(feed: &str) -> Option<FrameSize> {
    FEEDS.with(|cell| {
        cell.borrow()
            .shown
            .get(&FeedId::new(feed))
            .map(|entry| entry.wanted)
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
pub fn media_is_shown(host: Host) -> bool {
    FEEDS.with(|cell| cell.borrow().media_shown.contains(&host))
}

fn sink<B: UiBackend>(weak: slint::Weak<B::Window>, feed: FeedId, attachment: u64) -> FrameSink {
    let in_flight = Arc::new(AtomicBool::new(false));
    Arc::new(move |frame: &dyn VideoFrame| {
        if in_flight.swap(true, Ordering::AcqRel) {
            return;
        }
        let size = frame.size();
        let source = frame.source();
        let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(size.width, size.height);
        if !frame.write_rgba(pixels.make_mut_bytes()) {
            in_flight.store(false, Ordering::Release);
            return;
        }
        let landed = Arc::clone(&in_flight);
        let feed = feed.clone();
        let posted = weak.upgrade_in_event_loop(move |window| {
            landed.store(false, Ordering::Release);
            show::<B>(&window, &feed, attachment, pixels, source);
        });
        if posted.is_err() {
            in_flight.store(false, Ordering::Release);
        }
    })
}

fn show<B: UiBackend>(
    window: &B::Window,
    feed: &FeedId,
    attachment: u64,
    pixels: SharedPixelBuffer<Rgba8Pixel>,
    source: FrameSize,
) {
    let image = Image::from_rgba8(pixels);
    let (current, previewed) = FEEDS.with(|cell| {
        let mut state = cell.borrow_mut();
        let previewed = state.preview.feed.as_ref() == Some(feed);
        let current = match state.shown.get_mut(feed) {
            Some(entry) if entry.attachment == attachment => {
                entry.frame = Some(image.clone());
                entry.source = source;
                entry.frames = entry.frames.saturating_add(1);
                true
            }
            _ => false,
        };
        (current, previewed)
    });
    if !current {
        return;
    }
    if previewed {
        window.apply_call_preview(Some(image.clone()));
    }
    let models = B::models();
    show_in::<B>(&models.call_stage, feed, &image, source);
    show_in::<B>(&models.call_tiles, feed, &image, source);
}

fn show_in<B: UiBackend>(
    tiles: &VecModel<B::CallTile>,
    feed: &FeedId,
    image: &Image,
    source: FrameSize,
) {
    for row in 0..tiles.row_count() {
        let Some(mut tile) = tiles.row_data(row) else {
            continue;
        };
        if tile.feed() != feed.as_ref() {
            continue;
        }
        tile.set_frame(image.clone());
        tile.set_has_frame(true);
        put_source::<B>(&mut tile, source);
        tiles.set_row_data(row, tile);
    }
}
