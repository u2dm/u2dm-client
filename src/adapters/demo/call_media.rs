mod painted;
mod session;

use std::collections::{HashMap, HashSet};
use std::f64::consts::TAU;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{result, thread};

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use self::painted::{PORTRAIT_SOURCE, SCREEN_SOURCE, SENT_SCREEN_SOURCE, SOURCE, hue_of, paint};
use self::session::{DEMO_LEVEL_EVERY, DEMO_LEVEL_PERIOD, DemoMediaSession};
use super::call_devices::{DemoDevices, plugged_in};
use super::calls::{self, CAMERA_LOST_AFTER};
use crate::domain::call::device_test::{DeviceTest, TestNews};
use crate::domain::call::devices::{DeviceChoice, DeviceKind, DeviceLists};
use crate::domain::call::media::{MediaEvent, MediaSetup};
use crate::domain::call::video::{
    FeedId, FrameSize, Framing, PreviewFailure, ScreenRefusal, ScreenShare,
};
use crate::error::{AppError, Result};
use crate::ports::call::{
    CallMediaPort, CallVideoPort, CameraPreview, FrameSink, MediaSession, RunningTest,
};

const DEMO_FPS: u64 = 10;

const DEMO_TEST_SOUND: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct DemoCallMedia {
    showing: Mutex<HashMap<FeedId, Showing>>,
}

struct Showing {
    running: Arc<AtomicBool>,
    wanted: Arc<Mutex<FrameSize>>,
}

impl CallVideoPort for DemoCallMedia {
    fn attach(&self, feed: &FeedId, wanted: FrameSize, framing: Framing, sink: FrameSink) {
        self.detach(feed);
        let running = Arc::new(AtomicBool::new(true));
        let size = Arc::new(Mutex::new(wanted));
        let spawned = thread::Builder::new().name("demo-video".into()).spawn({
            let running = Arc::clone(&running);
            let size = Arc::clone(&size);
            let hue = hue_of(feed);
            let source = match framing {
                Framing::Contain if feed.as_ref() == calls::OWN_SCREEN_FEED => SENT_SCREEN_SOURCE,
                Framing::Contain => SCREEN_SOURCE,
                Framing::Cover if calls::is_portrait(feed) => PORTRAIT_SOURCE,
                Framing::Cover => SOURCE,
            };
            move || paint(&running, &size, source, framing, hue, &sink)
        });
        if let Err(e) = spawned {
            tracing::warn!("demo: no thread for a video: {e}");
            return;
        }
        tracing::info!(%feed, ?framing, "demo: showing a video");
        if let Ok(mut showing) = self.showing.lock() {
            showing.insert(
                feed.clone(),
                Showing {
                    running,
                    wanted: size,
                },
            );
        }
    }

    fn resize(&self, feed: &FeedId, wanted: FrameSize) {
        if let Ok(showing) = self.showing.lock()
            && let Some(shown) = showing.get(feed)
            && let Ok(mut size) = shown.wanted.lock()
        {
            *size = wanted;
            tracing::info!(%feed, width = wanted.width, height = wanted.height, "demo: a video is resized");
        }
    }

    fn detach(&self, feed: &FeedId) {
        if let Ok(mut showing) = self.showing.lock()
            && let Some(shown) = showing.remove(feed)
        {
            shown.running.store(false, Ordering::Release);
            tracing::info!(%feed, "demo: stopped showing a video");
        }
    }
}

#[async_trait]
impl CallMediaPort for DemoCallMedia {
    fn broken(&self) -> Option<String> {
        None
    }

    async fn list_devices(&self) -> Result<DeviceLists> {
        tracing::info!("demo: the call devices are listed without a call");
        Ok(plugged_in())
    }

    async fn preview_camera(
        &self,
        camera: DeviceChoice,
        lost: oneshot::Sender<String>,
    ) -> result::Result<Box<dyn CameraPreview>, PreviewFailure> {
        calls::pause().await;
        let lists = plugged_in();
        let Some(device) = camera.resolve(DeviceKind::Camera, &lists.connected.camera, None) else {
            tracing::info!("demo: there is no camera to preview");
            return Err(PreviewFailure::NoCamera);
        };
        if calls::scenario().camera_fails {
            tracing::info!(device = device.name, "demo: the camera to preview is busy");
            return Err(PreviewFailure::NotStarted(
                "demo: the camera is busy".into(),
            ));
        }
        tracing::info!(device = device.name, "demo: the camera preview starts");
        Ok(Box::new(DemoPreview {
            losing: calls::scenario().preview_lost.then(|| lose_preview(lost)),
        }))
    }

    async fn test_device(
        &self,
        test: DeviceTest,
        news: mpsc::UnboundedSender<TestNews>,
    ) -> Result<Box<dyn RunningTest>> {
        calls::pause().await;
        let kind = test.kind();
        if calls::scenario().audio_tests_fail {
            tracing::info!(%kind, "demo: the device test does not start");
            return Err(AppError::Other(
                "demo: the test connection did not connect".into(),
            ));
        }
        let (DeviceTest::Microphone(choice) | DeviceTest::Speaker(choice)) = &test;
        let lists = plugged_in();
        let device = choice
            .resolve(kind, lists.connected.of(kind), None)
            .map_or_else(
                || lists.defaults.of(kind).clone().unwrap_or_default(),
                |device| device.name.clone(),
            );
        tracing::info!(%kind, device, "demo: testing a device");
        let work = match test {
            DeviceTest::Microphone(_) => tokio::spawn(demo_levels(news)),
            DeviceTest::Speaker(_) => tokio::spawn(async move {
                sleep(DEMO_TEST_SOUND).await;
                tracing::info!("demo: the test sound ends");
                drop(news.send(TestNews::Played));
            }),
        };
        Ok(Box::new(DemoTest { work, kind }))
    }

    async fn open(
        &self,
        setup: MediaSetup,
        events_tx: mpsc::UnboundedSender<MediaEvent>,
    ) -> Result<Box<dyn MediaSession>> {
        tracing::info!(room_id = %setup.room_id, encrypted = setup.encrypted, "demo: the call media opens");
        if calls::scenario().no_screen_capture {
            tracing::info!("demo: this desktop has no screen-cast portal");
            let unavailable = MediaEvent::Screen(ScreenShare::Unavailable(ScreenRefusal::NoPortal));
            if events_tx.send(unavailable).is_err() {
                tracing::debug!("demo: the app stopped listening to the call media");
            }
        }
        let mut devices = DemoDevices::new(setup.devices);
        devices.shape_chosen(&events_tx);
        devices.refresh(&events_tx);
        Ok(Box::new(DemoMediaSession {
            room_id: setup.room_id,
            encrypted: setup.encrypted,
            events_tx,
            dialling: HashMap::new(),
            connected: HashMap::new(),
            microphone_published: false,
            microphone_on: false,
            microphone_samples: 0,
            camera_sends: None,
            connection_samples: 0,
            keyed: HashSet::new(),
            unheard: HashSet::new(),
            tasks: Vec::new(),
            screen: None,
            pickers: 0,
            devices,
        }))
    }
}

struct DemoPreview {
    losing: Option<JoinHandle<()>>,
}

struct DemoTest {
    work: JoinHandle<()>,
    kind: DeviceKind,
}

#[async_trait]
impl RunningTest for DemoTest {
    async fn close(self: Box<Self>) {
        self.work.abort();
        tracing::info!(kind = %self.kind, "demo: the device test stops");
    }
}

#[allow(clippy::cast_possible_truncation)]
async fn demo_levels(news: mpsc::UnboundedSender<TestNews>) {
    let mut step = 0.0_f64;
    loop {
        let level = 0.5 + 0.4 * (step * TAU / DEMO_LEVEL_PERIOD).sin();
        if news.send(TestNews::Level(level as f32)).is_err() {
            return;
        }
        step += 1.0;
        sleep(DEMO_LEVEL_EVERY).await;
    }
}

fn lose_preview(lost: oneshot::Sender<String>) -> JoinHandle<()> {
    tokio::spawn(async move {
        sleep(CAMERA_LOST_AFTER).await;
        tracing::info!("demo: the previewed camera stops sending pictures");
        if lost
            .send("demo: the camera stopped sending pictures".into())
            .is_err()
        {
            tracing::debug!("demo: the camera preview had already closed");
        }
    })
}

#[async_trait]
impl CameraPreview for DemoPreview {
    fn feed(&self) -> FeedId {
        FeedId::new(calls::PREVIEW_FEED)
    }

    async fn close(self: Box<Self>) {
        if let Some(losing) = &self.losing {
            losing.abort();
        }
        tracing::info!("demo: the camera preview stops");
    }
}
