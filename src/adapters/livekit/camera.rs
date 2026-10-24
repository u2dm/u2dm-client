use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoding, VideoPreset};
use livekit::prelude::{LocalTrack, LocalVideoTrack, Room, TrackSource};
use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use tokio::sync::{mpsc, oneshot};
use tokio::task::spawn_blocking;

use super::devices::listed_cameras;
use super::feeds::Feeds;
use crate::adapters::video::camera::{CameraDevice, Capture, Planes};
use crate::domain::call::devices::{DeviceChoice, DeviceKind};
use crate::domain::call::media::MediaEvent;
use crate::domain::call::video::{Camera, FeedId, PreviewFailure};
use crate::ports::call::CameraPreview;

pub(super) const OWN_FEED: &str = "own-camera";
const PREVIEW_FEED: &str = "camera-preview";
const TRACK_NAME: &str = "camera";
const CAPTURED: VideoResolution = VideoResolution {
    width: 1280,
    height: 720,
};
const FRAME_RATE: f64 = 30.0;
const TOP_BITRATE: u64 = 1_700_000;
const LOW_LAYER: (u32, u32, u64) = (320, 180, 160_000);
const MID_LAYER: (u32, u32, u64) = (640, 360, 450_000);

pub(super) struct OwnCamera {
    capture: Option<Capture>,
    track: LocalVideoTrack,
    source: NativeVideoSource,
    started: Instant,
    device: CameraDevice,
}

pub(super) fn own_feed() -> FeedId {
    FeedId::new(OWN_FEED)
}

pub(super) async fn start(
    room: &Room,
    device: CameraDevice,
    feeds: &Arc<Feeds>,
    events_tx: &mpsc::UnboundedSender<MediaEvent>,
) -> Result<OwnCamera, String> {
    tracing::info!(device = %device.path.display(), name = device.name, "starting the camera");
    let source = NativeVideoSource::new(CAPTURED, false);
    let started = Instant::now();
    let pump = frame_pump(source.clone(), Arc::clone(feeds), micros_since(started));
    let capture = capture(&device, pump, events_tx).await?;
    let track =
        LocalVideoTrack::create_video_track(TRACK_NAME, RtcVideoSource::Native(source.clone()));
    room.local_participant()
        .publish_track(LocalTrack::Video(track.clone()), publish_options())
        .await
        .map_err(|e| format!("the camera could not be published: {e}"))?;
    tracing::info!("published the camera");
    Ok(OwnCamera {
        capture: Some(capture),
        track,
        source,
        started,
        device,
    })
}

async fn capture(
    device: &CameraDevice,
    pump: impl FnMut(&Planes<'_>) + Send + 'static,
    events_tx: &mpsc::UnboundedSender<MediaEvent>,
) -> Result<Capture, String> {
    let lost_tx = events_tx.clone();
    let lost = move |why: String| {
        tracing::warn!("the camera stopped: {why}");
        if lost_tx
            .send(MediaEvent::Camera(Camera::Failed(why)))
            .is_err()
        {
            tracing::debug!("the app stopped listening to the camera");
        }
    };
    let device = device.clone();
    spawn_blocking(move || Capture::start(&device, pump, lost))
        .await
        .map_err(|e| format!("the camera did not start: {e}"))?
}

impl OwnCamera {
    pub(super) fn is_capturing(&self) -> bool {
        self.capture.as_ref().is_some_and(Capture::is_running)
    }

    pub(super) fn device_key(&self) -> &str {
        &self.device.key
    }

    pub(super) async fn switch_to(
        &mut self,
        device: CameraDevice,
        feeds: &Arc<Feeds>,
        events_tx: &mpsc::UnboundedSender<MediaEvent>,
    ) -> Result<(), String> {
        tracing::info!(device = %device.path.display(), name = device.name, "switching the camera");
        if let Some(previous) = self.capture.take() {
            release(previous).await;
        }
        let switched = capture(&device, self.pump(feeds), events_tx).await;
        match switched {
            Ok(started) => {
                self.capture = Some(started);
                self.device = device;
                Ok(())
            }
            Err(why) => {
                let previous = self.device.clone();
                match capture(&previous, self.pump(feeds), events_tx).await {
                    Ok(restarted) => self.capture = Some(restarted),
                    Err(again) => {
                        tracing::warn!("the previous camera did not start again: {again}");
                    }
                }
                Err(why)
            }
        }
    }

    fn pump(&self, feeds: &Arc<Feeds>) -> impl FnMut(&Planes<'_>) + Send + 'static {
        frame_pump(
            self.source.clone(),
            Arc::clone(feeds),
            micros_since(self.started),
        )
    }

    pub(super) async fn stop(mut self, room: Option<&Room>) {
        if let Some(capture) = self.capture.take() {
            release(capture).await;
        }
        if let Some(room) = room {
            self.unpublish(room).await;
        }
        tracing::info!("stopped the camera");
    }

    async fn unpublish(&self, room: &Room) {
        let unpublished = room
            .local_participant()
            .unpublish_track(&self.track.sid())
            .await;
        if let Err(e) = unpublished {
            tracing::debug!("the camera track did not unpublish: {e}");
        }
    }
}

pub(super) struct LocalPreview {
    capture: Option<Capture>,
}

pub(super) async fn preview(
    choice: DeviceChoice,
    feeds: Arc<Feeds>,
    lost: oneshot::Sender<String>,
) -> Result<LocalPreview, PreviewFailure> {
    let started = spawn_blocking(move || {
        let device = chosen_camera(&choice).ok_or(PreviewFailure::NoCamera)?;
        tracing::info!(device = %device.path.display(), name = device.name, "previewing the camera");
        let lost = move |why: String| {
            if lost.send(why).is_err() {
                tracing::debug!("the camera preview had already closed");
            }
        };
        Capture::start(&device, preview_pump(feeds), lost).map_err(PreviewFailure::NotStarted)
    })
    .await
    .map_err(|e| PreviewFailure::NotStarted(format!("the camera did not start: {e}")))?;
    Ok(LocalPreview {
        capture: Some(started?),
    })
}

fn chosen_camera(choice: &DeviceChoice) -> Option<CameraDevice> {
    let (cameras, listed) = listed_cameras();
    let chosen = choice.resolve(DeviceKind::Camera, &listed, None)?;
    cameras
        .into_iter()
        .find(|camera| camera.key == chosen.key.as_ref())
}

fn preview_pump(feeds: Arc<Feeds>) -> impl FnMut(&Planes<'_>) + Send + 'static {
    let feed = FeedId::new(PREVIEW_FEED);
    move |planes: &Planes<'_>| {
        let mut buffer = I420Buffer::new(planes.width, planes.height);
        copy_planes(&mut buffer, planes);
        feeds.push_local(&feed, &buffer);
    }
}

#[async_trait]
impl CameraPreview for LocalPreview {
    fn feed(&self) -> FeedId {
        FeedId::new(PREVIEW_FEED)
    }

    async fn close(mut self: Box<Self>) {
        if let Some(capture) = self.capture.take() {
            release(capture).await;
        }
        tracing::info!("stopped previewing the camera");
    }
}

fn micros_since(started: Instant) -> i64 {
    i64::try_from(started.elapsed().as_micros()).unwrap_or(i64::MAX)
}

async fn release(capture: Capture) {
    if spawn_blocking(move || drop(capture)).await.is_err() {
        tracing::debug!("the camera thread did not stop cleanly");
    }
}

fn frame_pump(
    source: NativeVideoSource,
    feeds: Arc<Feeds>,
    since_start_us: i64,
) -> impl FnMut(&Planes<'_>) + Send + 'static {
    let feed = own_feed();
    move |planes: &Planes<'_>| {
        let mut buffer = I420Buffer::new(planes.width, planes.height);
        copy_planes(&mut buffer, planes);
        let frame = VideoFrame {
            rotation: VideoRotation::VideoRotation0,
            timestamp_us: since_start_us.saturating_add(planes.timestamp_us),
            frame_metadata: None,
            buffer,
        };
        source.capture_frame(&frame);
        feeds.push_local(&feed, &frame.buffer);
    }
}

fn copy_planes(buffer: &mut I420Buffer, planes: &Planes<'_>) {
    let width = planes.width as usize;
    let height = planes.height as usize;
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    let (stride_y, stride_u, stride_v) = buffer.strides();
    let (y, u, v) = buffer.data_mut();
    copy_plane(y, stride_y as usize, planes.y, width, height);
    copy_plane(u, stride_u as usize, planes.u, chroma_width, chroma_height);
    copy_plane(v, stride_v as usize, planes.v, chroma_width, chroma_height);
}

fn copy_plane(to: &mut [u8], to_stride: usize, from: (&[u8], usize), width: usize, rows: usize) {
    let (from, from_stride) = from;
    if to_stride == 0 || from_stride == 0 {
        return;
    }
    for (to_row, from_row) in to
        .chunks_mut(to_stride)
        .zip(from.chunks(from_stride))
        .take(rows)
    {
        if let (Some(to_row), Some(from_row)) = (to_row.get_mut(..width), from_row.get(..width)) {
            to_row.copy_from_slice(from_row);
        }
    }
}

fn publish_options() -> TrackPublishOptions {
    TrackPublishOptions {
        source: TrackSource::Camera,
        video_codec: VideoCodec::VP8,
        video_encoding: Some(VideoEncoding {
            max_bitrate: TOP_BITRATE,
            max_framerate: FRAME_RATE,
        }),
        simulcast: true,
        simulcast_layers: Some(vec![layer(LOW_LAYER), layer(MID_LAYER)]),
        ..TrackPublishOptions::default()
    }
}

const fn layer((width, height, max_bitrate): (u32, u32, u64)) -> VideoPreset {
    VideoPreset {
        encoding: VideoEncoding {
            max_bitrate,
            max_framerate: FRAME_RATE,
        },
        width,
        height,
    }
}
