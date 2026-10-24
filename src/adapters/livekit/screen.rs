use std::env;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use glib::MainContext;
use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoding};
use livekit::prelude::{LocalTrack, LocalVideoTrack, Room, TrackSource};
use livekit::webrtc::desktop_capturer::{
    CaptureError, DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions, DesktopFrame,
};
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, spawn_blocking};
use tokio::time::error::Elapsed;
use tokio::time::timeout;
use tracing::{Instrument, Span};
use zbus::{Connection, Proxy};

use super::feeds::{Feeds, signed};
use crate::domain::call::media::MediaEvent;
use crate::domain::call::video::{FeedId, FrameSize, Framing, ScreenRefusal, ScreenShare};

const TRACK_NAME: &str = "screen";
const OWN_FEED: &str = "own-screen";
const LARGEST: FrameSize = FrameSize::new(1920, 1080);
const FRAME_RATE: f64 = 30.0;
const TOP_BITRATE: u64 = 5_000_000;
const TICK: Duration = Duration::from_millis(33);
const BGRA: u32 = 4;
const STOP_WAIT: Duration = Duration::from_secs(2);
const PORTAL_WAIT: Duration = Duration::from_secs(5);
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SCREEN_CAST: &str = "org.freedesktop.portal.ScreenCast";
const SOURCE_TYPES: &str = "AvailableSourceTypes";
const SESSION_TYPE: &str = "XDG_SESSION_TYPE";
const WAYLAND_SESSION: &str = "wayland";
const WAYLAND_DISPLAY: &str = "WAYLAND_DISPLAY";

pub(super) async fn refusal() -> Option<ScreenRefusal> {
    if !wayland_session() {
        tracing::info!("screen sharing needs a Wayland session");
        return Some(ScreenRefusal::NotWayland);
    }
    let why = portal_refusal(timeout(PORTAL_WAIT, source_types()).await)?;
    tracing::info!("screen sharing is unavailable: {why}");
    Some(ScreenRefusal::NoPortal)
}

fn portal_refusal(answer: Result<zbus::Result<u32>, Elapsed>) -> Option<String> {
    match answer {
        Ok(Ok(types)) if types != 0 => None,
        Ok(Ok(_)) => Some("the desktop portal can share no screen and no window".to_owned()),
        Ok(Err(e)) => Some(format!("no desktop portal answers: {e}")),
        Err(_) => Some("the desktop portal did not answer in time".to_owned()),
    }
}

fn wayland_session() -> bool {
    env::var(SESSION_TYPE).is_ok_and(|kind| kind.starts_with(WAYLAND_SESSION))
        && env::var_os(WAYLAND_DISPLAY).is_some()
}

async fn source_types() -> zbus::Result<u32> {
    let connection = Connection::session().await?;
    let proxy = Proxy::new(&connection, PORTAL, PORTAL_PATH, SCREEN_CAST).await?;
    proxy.get_property::<u32>(SOURCE_TYPES).await
}

pub(super) struct OwnScreen {
    stop_tx: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

pub(super) fn start(
    room: Arc<Room>,
    feeds: Arc<Feeds>,
    events_tx: mpsc::UnboundedSender<MediaEvent>,
) -> OwnScreen {
    let (stop_tx, stop_rx) = oneshot::channel();
    let task = tokio::spawn(share(room, feeds, events_tx, stop_rx).in_current_span());
    OwnScreen {
        stop_tx: Some(stop_tx),
        task,
    }
}

impl OwnScreen {
    pub(super) fn is_running(&self) -> bool {
        !self.task.is_finished()
    }

    pub(super) async fn stop(mut self) {
        if let Some(stop_tx) = self.stop_tx.take()
            && stop_tx.send(()).is_err()
        {
            tracing::debug!("the screen share had already ended");
        }
        if timeout(STOP_WAIT, &mut self.task).await.is_err() {
            tracing::warn!("the screen share did not stop in time");
            self.task.abort();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    NoCapturer,
    Refused,
    Closed { frames: u64 },
    Stopped { frames: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Asked,
    ByItself,
}

impl Stop {
    const fn report(self) -> Option<ScreenShare> {
        match self {
            Self::Asked => None,
            Self::ByItself => Some(ScreenShare::Off),
        }
    }
}

struct Share {
    events_tx: mpsc::UnboundedSender<MediaEvent>,
    stop_rx: oneshot::Receiver<()>,
    ended_rx: oneshot::Receiver<Ended>,
    capture: Capture,
}

async fn share(
    room: Arc<Room>,
    feeds: Arc<Feeds>,
    events_tx: mpsc::UnboundedSender<MediaEvent>,
    stop_rx: oneshot::Receiver<()>,
) {
    let outlet = Arc::new(Outlet::new(feeds));
    let Some((mut share, first_rx)) = Share::open(&outlet, events_tx, stop_rx) else {
        return;
    };
    let size = match share.first_frame(first_rx).await {
        Ok(size) => size,
        Err(stop) => {
            share.end(stop.report()).await;
            return;
        }
    };
    let track = match publish(&room, &outlet, size).await {
        Ok(track) => track,
        Err(why) => {
            share.end(Some(ScreenShare::Failed(why))).await;
            return;
        }
    };
    report(&share.events_tx, ScreenShare::On(outlet.preview.clone()));
    let stop = share.until_stopped().await;
    unpublish(&room, &track).await;
    share.end(stop.report()).await;
}

async fn publish(room: &Room, outlet: &Outlet, size: FrameSize) -> Result<LocalVideoTrack, String> {
    let source = NativeVideoSource::new(
        VideoResolution {
            width: size.width,
            height: size.height,
        },
        true,
    );
    outlet.install(source.clone());
    let track = LocalVideoTrack::create_video_track(TRACK_NAME, RtcVideoSource::Native(source));
    room.local_participant()
        .publish_track(LocalTrack::Video(track.clone()), publish_options())
        .await
        .map_err(|e| {
            tracing::warn!("the screen share could not be published: {e}");
            e.to_string()
        })?;
    tracing::info!(
        width = size.width,
        height = size.height,
        "sharing your screen"
    );
    Ok(track)
}

impl Share {
    fn open(
        outlet: &Arc<Outlet>,
        events_tx: mpsc::UnboundedSender<MediaEvent>,
        stop_rx: oneshot::Receiver<()>,
    ) -> Option<(Self, oneshot::Receiver<FrameSize>)> {
        let (first_tx, first_rx) = oneshot::channel();
        let (ended_tx, ended_rx) = oneshot::channel();
        match Capture::start(Arc::clone(outlet), first_tx, ended_tx) {
            Ok(capture) => {
                tracing::info!("asking the desktop what to share");
                let share = Self {
                    events_tx,
                    stop_rx,
                    ended_rx,
                    capture,
                };
                Some((share, first_rx))
            }
            Err(why) => {
                tracing::warn!("the screen share did not start: {why}");
                report(&events_tx, ScreenShare::Failed(why));
                None
            }
        }
    }

    async fn first_frame(
        &mut self,
        first_rx: oneshot::Receiver<FrameSize>,
    ) -> Result<FrameSize, Stop> {
        let first = tokio::select! {
            _ = &mut self.stop_rx => {
                tracing::info!("you stopped the screen share before choosing what to share");
                return Err(Stop::Asked);
            }
            ended = &mut self.ended_rx => {
                log_unstarted(ended.ok());
                return Err(Stop::ByItself);
            }
            size = first_rx => size,
        };
        if let Ok(size) = first {
            return Ok(size);
        }
        log_unstarted((&mut self.ended_rx).await.ok());
        Err(Stop::ByItself)
    }

    async fn until_stopped(&mut self) -> Stop {
        let ended = tokio::select! {
            _ = &mut self.stop_rx => None,
            ended = &mut self.ended_rx => Some(ended.ok()),
        };
        let Some(ended) = ended else {
            tracing::info!("stopping the screen share");
            return Stop::Asked;
        };
        log_ended(ended);
        Stop::ByItself
    }

    async fn end(mut self, screen: Option<ScreenShare>) {
        self.capture.stop().await;
        if let Ok(Ended::Stopped { frames }) = self.ended_rx.try_recv() {
            tracing::debug!(frames, "the screen capture stopped");
        }
        if let Some(screen) = screen {
            report(&self.events_tx, screen);
        }
    }
}

fn log_ended(ended: Option<Ended>) {
    if let Some(Ended::Closed { frames }) = ended {
        tracing::info!(frames, "the desktop ended the screen share");
    } else {
        tracing::info!(?ended, "the screen share ended");
    }
}

fn log_unstarted(ended: Option<Ended>) {
    match ended {
        Some(Ended::NoCapturer) => {
            tracing::warn!("the screen share did not start: no screen capturer for this desktop");
        }
        Some(Ended::Refused) => tracing::info!(
            "the screen share did not start: the picker was closed, or the desktop portal or PipeWire refused"
        ),
        other => tracing::info!(ended = ?other, "the screen share ended before its first picture"),
    }
}

fn report(events_tx: &mpsc::UnboundedSender<MediaEvent>, screen: ScreenShare) {
    if events_tx.send(MediaEvent::Screen(screen)).is_err() {
        tracing::debug!("the app stopped listening to the screen share");
    }
}

fn publish_options() -> TrackPublishOptions {
    TrackPublishOptions {
        source: TrackSource::Screenshare,
        video_codec: VideoCodec::VP8,
        video_encoding: Some(VideoEncoding {
            max_bitrate: TOP_BITRATE,
            max_framerate: FRAME_RATE,
        }),
        simulcast: true,
        ..TrackPublishOptions::default()
    }
}

async fn unpublish(room: &Room, track: &LocalVideoTrack) {
    let unpublished = room.local_participant().unpublish_track(&track.sid()).await;
    if let Err(e) = unpublished {
        tracing::debug!("the screen track did not unpublish: {e}");
    }
}

struct Outlet {
    source: Mutex<Option<NativeVideoSource>>,
    feeds: Arc<Feeds>,
    preview: FeedId,
}

impl Outlet {
    fn new(feeds: Arc<Feeds>) -> Self {
        Self {
            source: Mutex::new(None),
            feeds,
            preview: FeedId::new(OWN_FEED),
        }
    }

    fn install(&self, source: NativeVideoSource) {
        if let Ok(mut slot) = self.source.lock() {
            *slot = Some(source);
        }
    }

    fn send(&self, frame: &VideoFrame<I420Buffer>) {
        if let Ok(slot) = self.source.lock()
            && let Some(source) = slot.as_ref()
        {
            source.capture_frame(frame);
        }
        self.feeds.push_local(&self.preview, &frame.buffer);
    }
}

struct Capture {
    running: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Capture {
    fn start(
        outlet: Arc<Outlet>,
        first_tx: oneshot::Sender<FrameSize>,
        ended_tx: oneshot::Sender<Ended>,
    ) -> Result<Self, String> {
        let running = Arc::new(AtomicBool::new(true));
        let span = Span::current();
        let thread = thread::Builder::new()
            .name("u2dm-screen".into())
            .spawn({
                let running = Arc::clone(&running);
                move || {
                    let _in_call = span.enter();
                    let ended = capture_on_own_context(&running, outlet, first_tx);
                    if ended_tx.send(ended).is_err() {
                        tracing::debug!("nobody waits for the screen capture any more");
                    }
                }
            })
            .map_err(|e| format!("no thread for the screen share: {e}"))?;
        Ok(Self {
            running,
            thread: Some(thread),
        })
    }

    async fn stop(mut self) {
        self.running.store(false, Ordering::Release);
        let Some(thread) = self.thread.take() else {
            return;
        };
        let joined = spawn_blocking(move || thread.join().is_ok()).await;
        if !matches!(joined, Ok(true)) {
            tracing::warn!("the screen capture thread did not stop cleanly");
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

fn capture_on_own_context(
    running: &AtomicBool,
    outlet: Arc<Outlet>,
    first_tx: oneshot::Sender<FrameSize>,
) -> Ended {
    let context = MainContext::new();
    match context.with_thread_default(|| capture(&context, running, outlet, first_tx)) {
        Ok(ended) => ended,
        Err(e) => {
            tracing::warn!("the screen share has no GLib main context of its own: {e}");
            Ended::NoCapturer
        }
    }
}

fn capture(
    context: &MainContext,
    running: &AtomicBool,
    outlet: Arc<Outlet>,
    first_tx: oneshot::Sender<FrameSize>,
) -> Ended {
    let mut options = DesktopCapturerOptions::new(DesktopCaptureSourceType::Generic);
    options.set_include_cursor(true);
    let Some(mut capturer) = DesktopCapturer::new(options) else {
        return Ended::NoCapturer;
    };
    let pump = Arc::new(Pump {
        outlet,
        first_tx: Mutex::new(Some(first_tx)),
        frames: AtomicU64::new(0),
        ended: AtomicBool::new(false),
    });
    capturer.start_capture(None, {
        let pump = Arc::clone(&pump);
        move |result| pump.take(result)
    });
    while running.load(Ordering::Acquire) && !pump.ended.load(Ordering::Acquire) {
        while context.iteration(false) {}
        capturer.capture_frame();
        thread::sleep(TICK);
    }
    drop(capturer);
    while context.iteration(false) {}
    pump.outcome(running)
}

struct Pump {
    outlet: Arc<Outlet>,
    first_tx: Mutex<Option<oneshot::Sender<FrameSize>>>,
    frames: AtomicU64,
    ended: AtomicBool,
}

impl Pump {
    fn take(&self, result: Result<DesktopFrame, CaptureError>) {
        match result {
            Ok(frame) => self.frame(&frame),
            Err(CaptureError::Temporary) => {}
            Err(CaptureError::Permanent) => self.ended.store(true, Ordering::Release),
        }
    }

    fn frame(&self, frame: &DesktopFrame) {
        let Some(buffer) = converted(frame) else {
            return;
        };
        if self.frames.fetch_add(1, Ordering::AcqRel) == 0 {
            self.first(FrameSize::new(buffer.width(), buffer.height()));
        }
        self.outlet.send(&VideoFrame {
            rotation: VideoRotation::VideoRotation0,
            timestamp_us: 0,
            frame_metadata: None,
            buffer,
        });
    }

    fn first(&self, size: FrameSize) {
        let first_tx = self.first_tx.lock().ok().and_then(|mut slot| slot.take());
        if let Some(first_tx) = first_tx
            && first_tx.send(size).is_err()
        {
            tracing::debug!("nobody waits for the screen share's first picture");
        }
    }

    fn outcome(&self, running: &AtomicBool) -> Ended {
        let frames = self.frames.load(Ordering::Acquire);
        match (running.load(Ordering::Acquire), frames) {
            (false, _) => Ended::Stopped { frames },
            (true, 0) => Ended::Refused,
            (true, _) => Ended::Closed { frames },
        }
    }
}

fn converted(frame: &DesktopFrame) -> Option<I420Buffer> {
    let width = u32::try_from(frame.width()).ok().filter(|side| *side > 0)?;
    let height = u32::try_from(frame.height())
        .ok()
        .filter(|side| *side > 0)?;
    let stride = frame.stride();
    let data = frame.data();
    let needed = usize::try_from(u64::from(stride) * u64::from(height)).ok()?;
    if stride < width.checked_mul(BGRA)? || data.len() < needed {
        return None;
    }
    let mut i420 = I420Buffer::new(width, height);
    let (stride_y, stride_u, stride_v) = i420.strides();
    let (y, u, v) = i420.data_mut();
    yuv_helper::argb_to_i420(
        data,
        stride,
        y,
        stride_y,
        u,
        stride_u,
        v,
        stride_v,
        frame.width(),
        frame.height(),
    );
    let captured = FrameSize::new(width, height);
    let sent = captured.framed(LARGEST, Framing::Contain);
    Some(if sent == captured {
        i420
    } else {
        i420.scale(signed(sent.width), signed(sent.height))
    })
}
