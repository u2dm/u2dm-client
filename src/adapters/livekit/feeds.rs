use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use futures_util::StreamExt;
use livekit::prelude::{RemoteTrackPublication, RemoteVideoTrack, TrackDimension};
use livekit::webrtc::video_frame::native::VideoFrameBufferExt;
use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer, VideoFormatType, VideoRotation};
use livekit::webrtc::video_stream::native::NativeVideoStream;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::sfu::SfuId;
use crate::domain::call::video::{FeedId, FrameSize, Framing, VideoKind};
use crate::ports::call::{FrameSink, VideoFrame};

const RGBA: usize = 4;
const FEED_SEPARATOR: char = '|';
const CAMERA: &str = "camera";
const SCREEN: &str = "screen";

pub(super) fn feed_of(sfu: &SfuId, identity: &MediaIdentity, kind: VideoKind) -> FeedId {
    let source = match kind {
        VideoKind::Camera => CAMERA,
        VideoKind::Screen => SCREEN,
    };
    FeedId::new(format!(
        "{sfu}{FEED_SEPARATOR}{identity}{FEED_SEPARATOR}{source}"
    ))
}

#[derive(Default)]
pub(super) struct Feeds {
    entries: Mutex<HashMap<FeedId, Entry>>,
    runtime: Mutex<Option<Handle>>,
}

#[derive(Default)]
struct Entry {
    remote: Option<Remote>,
    attachment: Option<Attachment>,
    forwarder: Option<JoinHandle<()>>,
}

#[derive(Clone)]
struct Remote {
    track: RemoteVideoTrack,
    publication: RemoteTrackPublication,
}

#[derive(Clone)]
struct Attachment {
    sink: FrameSink,
    wanted: FrameSize,
    framing: Framing,
}

impl Feeds {
    pub(super) fn bind_runtime(&self, runtime: Handle) {
        if let Ok(mut slot) = self.runtime.lock() {
            *slot = Some(runtime);
        }
    }

    pub(super) fn add_remote(
        self: &Arc<Self>,
        feed: &FeedId,
        track: RemoteVideoTrack,
        publication: RemoteTrackPublication,
    ) {
        let remote = Remote { track, publication };
        let attached = self.with_entry(feed, |entry| {
            entry.remote = Some(remote.clone());
            entry.attachment.clone()
        });
        if let Some(attachment) = attached.flatten() {
            self.forward(feed, &remote, attachment.wanted);
        }
    }

    pub(super) fn remove_remote(&self, feed: &FeedId) {
        self.with_entry(feed, |entry| {
            entry.remote = None;
            stop(&mut entry.forwarder);
        });
        self.drop_unused(feed);
    }

    pub(super) fn push_local(&self, feed: &FeedId, buffer: &I420Buffer) {
        let Some(attachment) = self.attachment(feed) else {
            return;
        };
        let frame = Converted::new(buffer, VideoRotation::VideoRotation0, &attachment);
        (attachment.sink)(&frame);
    }

    pub(super) fn forget_server(&self, sfu: &SfuId) {
        let prefix = format!("{sfu}{FEED_SEPARATOR}");
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|feed, entry| {
                let keep = !feed.as_ref().starts_with(&prefix);
                if !keep {
                    stop(&mut entry.forwarder);
                }
                keep
            });
        }
    }

    pub(super) fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            for entry in entries.values_mut() {
                stop(&mut entry.forwarder);
            }
            entries.clear();
        }
    }

    fn attachment(&self, feed: &FeedId) -> Option<Attachment> {
        let entries = self.entries.lock().ok()?;
        entries.get(feed).and_then(|entry| entry.attachment.clone())
    }

    fn with_entry<T>(&self, feed: &FeedId, change: impl FnOnce(&mut Entry) -> T) -> Option<T> {
        let mut entries = self.entries.lock().ok()?;
        Some(change(entries.entry(feed.clone()).or_default()))
    }

    fn drop_unused(&self, feed: &FeedId) {
        if let Ok(mut entries) = self.entries.lock()
            && entries
                .get(feed)
                .is_some_and(|entry| entry.remote.is_none() && entry.attachment.is_none())
        {
            entries.remove(feed);
        }
    }

    fn forward(self: &Arc<Self>, feed: &FeedId, remote: &Remote, wanted: FrameSize) {
        remote
            .publication
            .update_video_dimensions(TrackDimension(wanted.width, wanted.height));
        let Some(runtime) = self.runtime.lock().ok().and_then(|slot| slot.clone()) else {
            tracing::warn!(%feed, "no runtime to show a call video on");
            return;
        };
        let task = runtime.spawn(forward_frames(
            Arc::downgrade(self),
            feed.clone(),
            remote.track.clone(),
        ));
        self.with_entry(feed, |entry| {
            stop(&mut entry.forwarder);
            entry.forwarder = Some(task);
        });
    }
}

impl Feeds {
    pub(super) fn attach(
        self: &Arc<Self>,
        feed: &FeedId,
        wanted: FrameSize,
        framing: Framing,
        sink: FrameSink,
    ) {
        let remote = self.with_entry(feed, |entry| {
            entry.attachment = Some(Attachment {
                sink,
                wanted,
                framing,
            });
            stop(&mut entry.forwarder);
            entry.remote.clone()
        });
        if let Some(remote) = remote.flatten() {
            self.forward(feed, &remote, wanted);
        }
    }

    pub(super) fn resize(&self, feed: &FeedId, wanted: FrameSize) {
        let remote = self.with_entry(feed, |entry| {
            if let Some(attachment) = entry.attachment.as_mut() {
                attachment.wanted = wanted;
            }
            entry.remote.clone()
        });
        if let Some(remote) = remote.flatten() {
            remote
                .publication
                .update_video_dimensions(TrackDimension(wanted.width, wanted.height));
        }
    }

    pub(super) fn detach(&self, feed: &FeedId) {
        self.with_entry(feed, |entry| {
            entry.attachment = None;
            stop(&mut entry.forwarder);
        });
        self.drop_unused(feed);
    }
}

fn stop(forwarder: &mut Option<JoinHandle<()>>) {
    if let Some(task) = forwarder.take() {
        task.abort();
    }
}

async fn forward_frames(feeds: Weak<Feeds>, feed: FeedId, track: RemoteVideoTrack) {
    let mut stream = NativeVideoStream::new(track.rtc_track());
    tracing::debug!(%feed, "receiving a call video");
    while let Some(frame) = stream.next().await {
        let Some(attachment) = feeds.upgrade().and_then(|feeds| feeds.attachment(&feed)) else {
            break;
        };
        let converted = Converted::new(frame.buffer.as_ref(), frame.rotation, &attachment);
        (attachment.sink)(&converted);
    }
    tracing::debug!(%feed, "stopped receiving a call video");
}

struct Converted<'a> {
    buffer: &'a dyn VideoBuffer,
    rotation: VideoRotation,
    shown: FrameSize,
    upright: FrameSize,
}

impl<'a> Converted<'a> {
    fn new(buffer: &'a dyn VideoBuffer, rotation: VideoRotation, attachment: &Attachment) -> Self {
        let turned = quarter_turned(rotation);
        let source = FrameSize::new(buffer.width(), buffer.height());
        let source_shown = if turned { swapped(source) } else { source };
        let shown = source_shown.framed(attachment.wanted, attachment.framing);
        let upright = if turned { swapped(shown) } else { shown };
        Self {
            buffer,
            rotation,
            shown,
            upright,
        }
    }
}

impl VideoFrame for Converted<'_> {
    fn size(&self) -> FrameSize {
        self.shown
    }

    fn write_rgba(&self, rgba: &mut [u8]) -> bool {
        if rgba.len() != self.shown.pixels() * RGBA || self.shown.pixels() == 0 {
            return false;
        }
        let mut i420 = self.buffer.to_i420();
        let source = FrameSize::new(i420.width(), i420.height());
        let scaled = if source == self.upright {
            i420
        } else {
            i420.scale(signed(self.upright.width), signed(self.upright.height))
        };
        let stride = self.upright.width * 4;
        if self.rotation == VideoRotation::VideoRotation0 {
            scaled.to_argb(
                VideoFormatType::ABGR,
                rgba,
                stride,
                signed(self.upright.width),
                signed(self.upright.height),
            );
            return true;
        }
        let mut upright = vec![0_u8; self.upright.pixels() * RGBA];
        scaled.to_argb(
            VideoFormatType::ABGR,
            &mut upright,
            stride,
            signed(self.upright.width),
            signed(self.upright.height),
        );
        rotate(&upright, self.upright, self.rotation, rgba)
    }
}

const fn quarter_turned(rotation: VideoRotation) -> bool {
    matches!(
        rotation,
        VideoRotation::VideoRotation90 | VideoRotation::VideoRotation270
    )
}

const fn swapped(size: FrameSize) -> FrameSize {
    FrameSize::new(size.height, size.width)
}

pub(super) fn signed(side: u32) -> i32 {
    i32::try_from(side).unwrap_or(i32::MAX)
}

fn rotate(upright: &[u8], size: FrameSize, rotation: VideoRotation, out: &mut [u8]) -> bool {
    let (width, height) = (size.width as usize, size.height as usize);
    for (index, pixel) in upright.chunks_exact(RGBA).enumerate() {
        let (x, y) = (index % width, index / width);
        let (to_x, to_y, to_width) = match rotation {
            VideoRotation::VideoRotation0 => (x, y, width),
            VideoRotation::VideoRotation90 => (height - 1 - y, x, height),
            VideoRotation::VideoRotation180 => (width - 1 - x, height - 1 - y, width),
            VideoRotation::VideoRotation270 => (y, width - 1 - x, height),
        };
        let start = (to_y * to_width + to_x) * RGBA;
        let Some(target) = out.get_mut(start..start + RGBA) else {
            return false;
        };
        target.copy_from_slice(pixel);
    }
    true
}
