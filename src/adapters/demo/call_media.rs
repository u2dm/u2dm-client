use std::collections::{HashMap, HashSet};
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::thread;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio_util::task::AbortOnDropHandle;
use tracing::Instrument;

use super::call_devices::DemoDevices;
use super::calls::{
    self, CAMERA_LOST_AFTER, CUT_OFF_AFTER, PICKER_TIME, RECONNECT_AFTER, RECONNECT_FOR,
    SHARE_ENDS_AFTER, SPEAKING_TURN, UNTRUSTED,
};
use super::data;
use crate::domain::call::devices::{DeviceChoice, DeviceKind};
use crate::domain::call::identity::{MediaIdentity, spellings};
use crate::domain::call::keys::{KeyIndex, MediaKey};
use crate::domain::call::media::{Decryption, MediaEvent, MediaSetup, Microphone, SfuState};
use crate::domain::call::sfu::{SfuGrant, SfuId, SfuRole};
use crate::domain::call::video::{Camera, FeedId, FrameSize, Framing, ScreenRefusal, ScreenShare};
use crate::domain::room::RoomId;
use crate::error::Result;
use crate::ports::call::{
    CallMediaPort, CallVideoPort, ConnectedSfu, FrameSink, MediaSession, VideoFrame,
};

const DEMO_FPS: u64 = 10;
const SOURCE: FrameSize = FrameSize::new(1280, 720);
const PORTRAIT_SOURCE: FrameSize = FrameSize::new(720, 1280);
const SCREEN_SOURCE: FrameSize = FrameSize::new(2560, 1440);
const BAR_STEP: u64 = 6;
const BAR_SHARE: u32 = 12;
const WINDOW_INSET: u32 = 8;
const TITLE_SHARE: u32 = 16;
const DESKTOP: [u8; 3] = [0x2f, 0x3b, 0x52];
const WINDOW: [u8; 3] = [0xf2, 0xf2, 0xee];
const TITLE: [u8; 3] = [0x4c, 0x6e, 0xa8];

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

fn paint(
    running: &AtomicBool,
    size: &Mutex<FrameSize>,
    source: FrameSize,
    framing: Framing,
    hue: [u8; 3],
    sink: &FrameSink,
) {
    let mut frame_number = 0_u64;
    while running.load(Ordering::Acquire) {
        let wanted = size.lock().map_or(source, |size| *size);
        let painted = Painted {
            size: source.framed(wanted, framing),
            look: match framing {
                Framing::Cover => Look::Camera(hue),
                Framing::Contain => Look::Screen,
            },
            frame_number,
        };
        sink(&painted);
        frame_number = frame_number.wrapping_add(1);
        thread::sleep(Duration::from_millis(1000 / DEMO_FPS));
    }
}

fn hue_of(feed: &FeedId) -> [u8; 3] {
    let sum = feed.as_ref().bytes().fold(0_u32, |sum, byte| {
        sum.wrapping_mul(31).wrapping_add(u32::from(byte))
    });
    let [a, b, c, _] = sum.to_le_bytes();
    [a | 0x40, b | 0x40, c | 0x40]
}

#[derive(Clone, Copy)]
enum Look {
    Camera([u8; 3]),
    Screen,
}

struct Painted {
    size: FrameSize,
    look: Look,
    frame_number: u64,
}

impl Painted {
    fn colour_at(&self, x: u32, y: u32, bar_x: u32, bar_width: u32) -> [u8; 3] {
        let lit = x >= bar_x && x < bar_x.saturating_add(bar_width);
        match self.look {
            Look::Camera(_) if lit => [u8::MAX; 3],
            Look::Camera(hue) => hue,
            Look::Screen => {
                let width = self.size.width.max(1);
                let height = self.size.height.max(1);
                let inset_x = width / WINDOW_INSET;
                let inset_y = height / WINDOW_INSET;
                let inside =
                    x >= inset_x && x < width - inset_x && y >= inset_y && y < height - inset_y;
                let title = y < inset_y + height / TITLE_SHARE;
                if !inside {
                    DESKTOP
                } else if title || lit {
                    TITLE
                } else {
                    WINDOW
                }
            }
        }
    }
}

impl VideoFrame for Painted {
    fn size(&self) -> FrameSize {
        self.size
    }

    fn write_rgba(&self, rgba: &mut [u8]) -> bool {
        let width = self.size.width.max(1);
        let bar_width = (width / BAR_SHARE).max(1);
        let bar_x =
            u32::try_from(self.frame_number.wrapping_mul(BAR_STEP) % u64::from(width)).unwrap_or(0);
        for (index, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let index = u32::try_from(index).unwrap_or(0);
            let [red, green, blue] = self.colour_at(index % width, index / width, bar_x, bar_width);
            *pixel = [red, green, blue, u8::MAX];
        }
        true
    }
}

#[async_trait]
impl CallMediaPort for DemoCallMedia {
    fn broken(&self) -> Option<String> {
        None
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
        devices.refresh(&events_tx);
        Ok(Box::new(DemoMediaSession {
            room_id: setup.room_id,
            encrypted: setup.encrypted,
            events_tx,
            dialling: HashMap::new(),
            connected: HashMap::new(),
            microphone_published: false,
            keyed: HashSet::new(),
            tasks: Vec::new(),
            screen: None,
            devices,
        }))
    }
}

type DemoDial = AbortOnDropHandle<result::Result<SfuGrant, String>>;

struct DemoMediaSession {
    room_id: RoomId,
    encrypted: bool,
    events_tx: mpsc::UnboundedSender<MediaEvent>,
    dialling: HashMap<SfuId, DemoDial>,
    connected: HashMap<SfuId, SfuRole>,
    microphone_published: bool,
    keyed: HashSet<MediaIdentity>,
    tasks: Vec<JoinHandle<()>>,
    screen: Option<JoinHandle<()>>,
    devices: DemoDevices,
}

impl DemoMediaSession {
    fn emit(&self, event: MediaEvent) {
        if self.events_tx.send(event).is_err() {
            tracing::debug!("demo: the app stopped listening to the call media");
        }
    }

    fn own_identity() -> MediaIdentity {
        let user = data::own_user();
        let device = calls::own_device();
        let [legacy, _, _] = spellings(user, device, &format!("{user}:{device}"));
        legacy
    }

    fn start_screen_share(&mut self) {
        if self
            .screen
            .as_ref()
            .is_some_and(|screen| !screen.is_finished())
        {
            return;
        }
        let events_tx = self.events_tx.clone();
        let shaped = calls::scenario();
        self.screen = Some(tokio::spawn(async move {
            tracing::info!("demo: the picker for your screen share opens");
            sleep(PICKER_TIME).await;
            if shaped.share_cancelled {
                tracing::info!("demo: the picker is closed without a choice");
                drop(events_tx.send(MediaEvent::Screen(ScreenShare::Off)));
                return;
            }
            tracing::info!("demo: you share your screen");
            drop(events_tx.send(MediaEvent::Screen(ScreenShare::On)));
            if shaped.share_ends {
                sleep(SHARE_ENDS_AFTER).await;
                tracing::info!("demo: the desktop ends your screen share");
                drop(events_tx.send(MediaEvent::Screen(ScreenShare::Off)));
            }
        }));
    }

    fn stop_screen_share(&mut self) {
        if let Some(screen) = self.screen.take() {
            screen.abort();
        }
        tracing::info!("demo: you stop sharing your screen");
    }

    fn spawn_reconnect(&mut self, sfu: SfuId) {
        let events_tx = self.events_tx.clone();
        self.tasks.push(tokio::spawn(async move {
            sleep(RECONNECT_AFTER).await;
            tracing::info!(%sfu, "demo: the media server reconnects");
            drop(events_tx.send(MediaEvent::Sfu {
                sfu: sfu.clone(),
                state: SfuState::Reconnecting,
            }));
            sleep(RECONNECT_FOR).await;
            drop(events_tx.send(MediaEvent::Sfu {
                sfu,
                state: SfuState::Connected,
            }));
        }));
    }

    fn spawn_cut_off(&mut self, sfu: SfuId) {
        let events_tx = self.events_tx.clone();
        self.tasks.push(tokio::spawn(async move {
            sleep(CUT_OFF_AFTER).await;
            tracing::info!(%sfu, "demo: the media server drops your connection");
            drop(events_tx.send(MediaEvent::Sfu {
                sfu,
                state: SfuState::Disconnected("demo: the media server dropped you".into()),
            }));
        }));
    }

    fn spawn_camera_loss(&mut self) {
        let events_tx = self.events_tx.clone();
        self.tasks.push(tokio::spawn(async move {
            sleep(CAMERA_LOST_AFTER).await;
            tracing::info!("demo: your camera stops sending pictures");
            drop(events_tx.send(MediaEvent::Camera(Camera::Failed(
                "demo: the camera stopped sending pictures".into(),
            ))));
        }));
    }

    fn spawn_speaking(&mut self, sfu: SfuId) {
        let events_tx = self.events_tx.clone();
        let room_id = self.room_id.clone();
        self.tasks.push(tokio::spawn(async move {
            let mut turn = 0_usize;
            loop {
                sleep(SPEAKING_TURN).await;
                let mut voices: Vec<MediaIdentity> = calls::book()
                    .others_in(room_id.as_ref())
                    .into_iter()
                    .map(|(_, identity)| identity)
                    .collect();
                voices.push(Self::own_identity());
                let speaker = voices.get(turn % voices.len().max(1)).cloned();
                turn = turn.wrapping_add(1);
                let speaking = MediaEvent::Speaking {
                    sfu: sfu.clone(),
                    speakers: speaker.into_iter().collect(),
                };
                if events_tx.send(speaking).is_err() {
                    return;
                }
            }
        }));
    }

    fn publishes(&self) -> bool {
        self.connected
            .values()
            .any(|role| *role == SfuRole::Publish)
    }

    fn finished_dial(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<(SfuId, result::Result<SfuGrant, String>)> {
        let ready = self.dialling.iter_mut().find_map(|(sfu, dialling)| {
            match Pin::new(dialling).poll(cx) {
                Poll::Ready(joined) => Some((sfu.clone(), joined)),
                Poll::Pending => None,
            }
        });
        let Some((sfu, joined)) = ready else {
            return Poll::Pending;
        };
        self.dialling.remove(&sfu);
        let dialled = joined.unwrap_or_else(|e| Err(format!("demo: the connection stopped: {e}")));
        Poll::Ready((sfu, dialled))
    }

    fn connected_to(&mut self, grant: SfuGrant) -> ConnectedSfu {
        let SfuGrant {
            sfu,
            role,
            identity,
            ..
        } = grant;
        tracing::info!(%sfu, ?role, %identity, "demo: connected to a media server");
        self.emit(MediaEvent::Sfu {
            sfu: sfu.clone(),
            state: SfuState::Connected,
        });
        calls::book().watch(self.room_id.as_ref(), sfu.as_ref(), self.events_tx.clone());
        self.connected.insert(sfu.clone(), role);
        self.report_waiting();
        if role == SfuRole::Publish {
            if calls::scenario().reconnecting {
                self.spawn_reconnect(sfu.clone());
            }
            if calls::cuts_off() {
                self.spawn_cut_off(sfu.clone());
            }
            if calls::scenario().speaking {
                self.spawn_speaking(sfu.clone());
            }
        }
        ConnectedSfu { sfu, role }
    }

    fn report_waiting(&self) {
        if !self.encrypted {
            return;
        }
        let untrusted = calls::scenario().undecryptable;
        for (member, identity) in calls::book().others_in(self.room_id.as_ref()) {
            if self.keyed.contains(&identity) {
                continue;
            }
            let state = if untrusted && member.user == UNTRUSTED {
                Decryption::Failed
            } else {
                Decryption::MissingKey
            };
            self.emit(MediaEvent::Decryption { identity, state });
        }
    }
}

#[async_trait]
impl MediaSession for DemoMediaSession {
    fn dial(&mut self, grant: SfuGrant) {
        let sfu = grant.sfu.clone();
        if self.connected.contains_key(&sfu) || self.dialling.contains_key(&sfu) {
            tracing::debug!(%sfu, "demo: already connected or connecting to this media server");
            return;
        }
        tracing::info!(%sfu, role = ?grant.role, "demo: connecting to a media server");
        let dialled = tokio::spawn(
            async move {
                calls::pause().await;
                if calls::scenario().connect_fails {
                    return Err("demo: the media server refused".to_owned());
                }
                Ok(grant)
            }
            .in_current_span(),
        );
        self.dialling.insert(sfu, AbortOnDropHandle::new(dialled));
    }

    async fn next_connected(&mut self) -> ConnectedSfu {
        loop {
            let (sfu, dialled) = poll_fn(|cx| self.finished_dial(cx)).await;
            match dialled {
                Ok(grant) => return self.connected_to(grant),
                Err(why) => {
                    tracing::info!(%sfu, "demo: the media server refuses the connection");
                    self.emit(MediaEvent::Sfu {
                        sfu,
                        state: SfuState::Disconnected(why),
                    });
                }
            }
        }
    }

    async fn disconnect(&mut self, sfu: &SfuId) {
        tracing::info!(%sfu, "demo: left a media server");
        self.dialling.remove(sfu);
        self.connected.remove(sfu);
    }

    async fn set_microphone(&mut self, on: bool) {
        if self.microphone_published {
            tracing::info!(on, "demo: the microphone switches");
            self.emit(MediaEvent::Microphone(if on {
                Microphone::On
            } else {
                Microphone::Off
            }));
        } else if on && self.publishes() {
            self.microphone_published = true;
            tracing::info!("demo: published the microphone");
            self.emit(MediaEvent::Microphone(Microphone::On));
        } else if on {
            tracing::info!("demo: the microphone waits for the call to connect");
        }
    }

    async fn set_camera(&mut self, on: bool) {
        calls::pause().await;
        if !on {
            tracing::info!(camera = "off", "demo: the camera switches");
            self.devices.stop_camera();
            return;
        }
        if !self.publishes() {
            tracing::info!("demo: the camera waits for the call to connect");
            return;
        }
        let camera = match self.devices.start_camera() {
            Err(why) => Camera::Failed(why),
            Ok(_) if calls::scenario().camera_fails => {
                self.devices.stop_camera();
                Camera::Failed("demo: the camera is busy".into())
            }
            Ok(device) => {
                tracing::info!(device, "demo: the camera uses a device");
                Camera::On(FeedId::new(calls::OWN_CAMERA_FEED))
            }
        };
        tracing::info!(%camera, "demo: the camera switches");
        let started = matches!(camera, Camera::On(_));
        self.emit(MediaEvent::Camera(camera));
        if started && calls::loses_camera() {
            self.spawn_camera_loss();
        }
    }

    async fn set_screen_share(&mut self, on: bool) {
        calls::pause().await;
        if on {
            self.start_screen_share();
        } else {
            self.stop_screen_share();
        }
    }

    async fn use_device(&mut self, kind: DeviceKind, choice: DeviceChoice) {
        calls::pause().await;
        self.devices.choose(kind, choice, &self.events_tx);
    }

    async fn refresh_devices(&mut self) {
        self.devices.refresh(&self.events_tx);
    }

    async fn set_own_key(&mut self, index: KeyIndex, _key: &MediaKey) {
        tracing::info!(%index, "demo: your call key is installed");
    }

    async fn use_own_key(&mut self, index: KeyIndex) {
        tracing::info!(%index, "demo: your call media uses a new key");
    }

    async fn set_key(&mut self, identities: &[MediaIdentity], index: KeyIndex, _key: &MediaKey) {
        let others = calls::book().others_in(self.room_id.as_ref());
        let Some((member, identity)) = others
            .into_iter()
            .find(|(_, identity)| identities.contains(identity))
        else {
            tracing::info!(
                identities = identities.len(),
                "demo: a call key for nobody in the call"
            );
            return;
        };
        tracing::info!(%member, %index, "demo: a call key is installed");
        let state = if calls::scenario().undecryptable && member.user == UNTRUSTED {
            Decryption::Failed
        } else {
            Decryption::Ok
        };
        self.keyed.insert(identity.clone());
        self.emit(MediaEvent::Decryption { identity, state });
    }

    async fn close(self: Box<Self>) {
        tracing::info!(room_id = %self.room_id, "demo: the call media closes");
        for task in self.tasks.iter().chain(&self.screen) {
            task.abort();
        }
        self.devices.close();
    }
}
