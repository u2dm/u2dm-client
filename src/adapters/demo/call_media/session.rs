use std::collections::{HashMap, HashSet};
use std::f64::consts::TAU;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::result;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, sleep_until};
use tokio_util::task::AbortOnDropHandle;
use tracing::Instrument;

use super::painted::{PORTRAIT_SOURCE, SENT_SCREEN_SOURCE, SOURCE};
use crate::adapters::demo::call_devices::DemoDevices;
use crate::adapters::demo::calls::{
    self, CAMERA_LOST_AFTER, CUT_OFF_AFTER, NOT_ENCRYPTED_AFTER, PICKER_TIME, RECONNECT_AFTER,
    RECONNECT_FOR, SHARE_ENDS_AFTER, SPEAKING_TURN, Trouble, UNTRUSTED,
};
use crate::adapters::demo::data;
use crate::domain::call::device_test::loudness_percent;
use crate::domain::call::devices::{DeviceChoice, DeviceKind};
use crate::domain::call::identity::{MediaIdentity, spellings};
use crate::domain::call::keys::{KeyIndex, MediaKey};
use crate::domain::call::media::{Decryption, EncryptionTrouble, MediaEvent, Microphone, SfuState};
use crate::domain::call::sfu::{SfuGrant, SfuId, SfuRole};
use crate::domain::call::stats::{ConnectionReading, ReceivedVideo, SentVideo};
use crate::domain::call::video::{Camera, FeedId, FrameSize, ScreenShare, VideoKind};
use crate::domain::room::RoomId;
use crate::ports::call::{ConnectedSfu, MediaSession};

pub(super) const DEMO_LEVEL_EVERY: Duration = Duration::from_millis(100);

pub(super) const DEMO_LEVEL_PERIOD: f64 = 20.0;

const DEMO_SENT_BYTES: u64 = 80_000;

const DEMO_RECEIVED_BYTES: u64 = 180_000;

const DEMO_PACKETS: u64 = 400;

const DEMO_LOST_PACKETS: u64 = 1;

const DEMO_ROUND_TRIP_MS: u32 = 48;

type DemoDial = AbortOnDropHandle<result::Result<SfuGrant, String>>;

pub(super) struct DemoMediaSession {
    pub(super) room_id: RoomId,
    pub(super) encrypted: bool,
    pub(super) events_tx: mpsc::UnboundedSender<MediaEvent>,
    pub(super) dialling: HashMap<SfuId, DemoDial>,
    pub(super) connected: HashMap<SfuId, SfuRole>,
    pub(super) microphone_published: bool,
    pub(super) microphone_on: bool,
    pub(super) microphone_samples: u32,
    pub(super) camera_sends: Option<FrameSize>,
    pub(super) connection_samples: u64,
    pub(super) keyed: HashSet<MediaIdentity>,
    pub(super) unheard: HashSet<MediaIdentity>,
    pub(super) tasks: Vec<JoinHandle<()>>,
    pub(super) screen: Option<JoinHandle<()>>,
    pub(super) pickers: u32,
    pub(super) devices: DemoDevices,
}

impl DemoMediaSession {
    fn emit(&self, event: MediaEvent) {
        if self.events_tx.send(event).is_err() {
            tracing::debug!("demo: the app stopped listening to the call media");
        }
    }

    fn sent_videos(&self) -> Vec<SentVideo> {
        let camera = self.camera_sends.map(|size| SentVideo {
            kind: VideoKind::Camera,
            size,
        });
        let screen = self.screen.is_some().then_some(SentVideo {
            kind: VideoKind::Screen,
            size: SENT_SCREEN_SOURCE,
        });
        camera.into_iter().chain(screen).collect()
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
        self.pickers = self.pickers.saturating_add(1);
        let cancelled = shaped.share_cancelled || (shaped.change_cancelled && self.pickers > 1);
        self.screen = Some(tokio::spawn(async move {
            tracing::info!("demo: the picker for your screen share opens");
            sleep(PICKER_TIME).await;
            if cancelled {
                tracing::info!("demo: the picker is closed without a choice");
                drop(events_tx.send(MediaEvent::Screen(ScreenShare::Off)));
                return;
            }
            tracing::info!("demo: you share your screen");
            let own = FeedId::new(calls::OWN_SCREEN_FEED);
            drop(events_tx.send(MediaEvent::Screen(ScreenShare::On(own))));
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
        let (gone, back) = calls::book().restart_news(self.room_id.as_ref(), sfu.as_ref());
        self.tasks.push(tokio::spawn(async move {
            sleep(RECONNECT_AFTER).await;
            tracing::info!(%sfu, full = !gone.is_empty(), "demo: the media server reconnects");
            for event in gone {
                drop(events_tx.send(event));
            }
            drop(events_tx.send(MediaEvent::Sfu {
                sfu: sfu.clone(),
                state: SfuState::Reconnecting,
            }));
            sleep(RECONNECT_FOR).await;
            drop(events_tx.send(MediaEvent::Sfu {
                sfu,
                state: SfuState::Connected,
            }));
            for event in back {
                drop(events_tx.send(event));
            }
        }));
    }

    fn spawn_troubles(&mut self, troubles: Vec<Trouble>) {
        if troubles.is_empty() {
            return;
        }
        let events_tx = self.events_tx.clone();
        self.tasks.push(tokio::spawn(async move {
            let connected = Instant::now();
            for trouble in troubles {
                sleep_until(connected + trouble.after).await;
                tracing::info!("{}", trouble.says);
                drop(events_tx.send(trouble.event));
            }
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

    fn spawn_encryption_failure(&mut self, sfu: SfuId) {
        let events_tx = self.events_tx.clone();
        self.tasks.push(tokio::spawn(async move {
            sleep(NOT_ENCRYPTED_AFTER).await;
            tracing::info!(%sfu, "demo: your call media cannot be encrypted");
            drop(events_tx.send(MediaEvent::NotEncrypted {
                sfu,
                trouble: EncryptionTrouble::Failed,
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

    fn spawn_speeches(&mut self, sfu: SfuId) {
        let events_tx = self.events_tx.clone();
        let room_id = self.room_id.clone();
        self.tasks.push(tokio::spawn(async move {
            loop {
                for (user, length) in calls::SPEECHES {
                    let speakers: Vec<MediaIdentity> = calls::book()
                        .others_in(room_id.as_ref())
                        .into_iter()
                        .filter(|(member, _)| member.user == user)
                        .map(|(_, identity)| identity)
                        .take(1)
                        .collect();
                    tracing::info!(user, "demo: someone speaks");
                    let speaking = MediaEvent::Speaking {
                        sfu: sfu.clone(),
                        speakers,
                    };
                    if events_tx.send(speaking).is_err() {
                        return;
                    }
                    sleep(length).await;
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
        self.spawn_troubles(calls::book().troubles(self.room_id.as_ref(), sfu.as_ref(), role));
        if role == SfuRole::Publish {
            if calls::scenario().reconnecting {
                self.spawn_reconnect(sfu.clone());
            }
            if calls::cuts_off() {
                self.spawn_cut_off(sfu.clone());
            }
            if self.encrypted && calls::fails_to_encrypt() {
                self.spawn_encryption_failure(sfu.clone());
            }
            if calls::scenario().speaking {
                self.spawn_speaking(sfu.clone());
            }
            if calls::scenario().speeches {
                self.spawn_speeches(sfu.clone());
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
        self.microphone_on = on;
        if self.microphone_published {
            tracing::info!(on, "demo: the microphone switches");
            self.emit(MediaEvent::Microphone(if on {
                Microphone::On
            } else {
                Microphone::Off
            }));
        } else if on && self.publishes() && calls::scenario().microphone_fails {
            tracing::info!("demo: the microphone is busy");
            self.emit(MediaEvent::Microphone(Microphone::Failed(
                "demo: the microphone is busy".into(),
            )));
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
            self.camera_sends = None;
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
        self.camera_sends = started.then_some(SOURCE);
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

    #[allow(clippy::cast_possible_truncation)]
    async fn sample_microphone(&mut self) {
        if !self.microphone_published || !self.microphone_on {
            return;
        }
        let step = f64::from(self.microphone_samples);
        self.microphone_samples = self.microphone_samples.wrapping_add(1);
        let loudness = 0.5 + 0.4 * (step * TAU / DEMO_LEVEL_PERIOD).sin();
        self.emit(MediaEvent::MicrophoneLevel(loudness_percent(
            loudness as f32,
        )));
    }

    async fn sample_connections(&mut self) {
        self.connection_samples = self.connection_samples.saturating_add(1);
        let samples = self.connection_samples;
        let readings = self
            .connected
            .iter()
            .map(|(sfu, role)| {
                let publishes = *role == SfuRole::Publish;
                ConnectionReading {
                    sfu: sfu.clone(),
                    sent_bytes: publishes.then(|| samples.saturating_mul(DEMO_SENT_BYTES)),
                    received_bytes: Some(samples.saturating_mul(DEMO_RECEIVED_BYTES)),
                    received_packets: Some(samples.saturating_mul(DEMO_PACKETS)),
                    lost_packets: Some(samples.saturating_mul(DEMO_LOST_PACKETS)),
                    round_trip_ms: Some(DEMO_ROUND_TRIP_MS),
                    sent_video: if publishes {
                        self.sent_videos()
                    } else {
                        Vec::new()
                    },
                    received_video: calls::book()
                        .cameras_on(self.room_id.as_ref(), sfu.as_ref())
                        .into_iter()
                        .map(|(identity, feed)| ReceivedVideo {
                            identity,
                            kind: VideoKind::Camera,
                            size: if calls::is_portrait(&feed) {
                                PORTRAIT_SOURCE
                            } else {
                                SOURCE
                            },
                        })
                        .collect(),
                }
            })
            .collect();
        self.emit(MediaEvent::Connections(readings));
    }

    async fn set_unheard(&mut self, identities: &[MediaIdentity]) {
        let unheard: HashSet<MediaIdentity> = identities.iter().cloned().collect();
        for (member, identity) in calls::book().others_in(self.room_id.as_ref()) {
            match (
                self.unheard.contains(&identity),
                unheard.contains(&identity),
            ) {
                (false, true) => {
                    tracing::info!(%member, "demo: you no longer hear someone in the call");
                }
                (true, false) => {
                    tracing::info!(%member, "demo: you hear someone in the call again");
                }
                _ => {}
            }
        }
        self.unheard = unheard;
    }

    async fn close(self: Box<Self>) {
        tracing::info!(room_id = %self.room_id, "demo: the call media closes");
        for task in self.tasks.iter().chain(&self.screen) {
            task.abort();
        }
        self.devices.close();
    }
}
