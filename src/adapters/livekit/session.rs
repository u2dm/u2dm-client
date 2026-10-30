use std::collections::HashMap;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use livekit::e2ee::key_provider::KeyProvider;
use livekit::id::ParticipantIdentity;
use livekit::options::TrackPublishOptions;
use livekit::prelude::{LocalAudioTrack, LocalTrack, PlatformAudio, TrackSource};
use livekit::webrtc::stats::RtcStats;
use tokio::sync::mpsc;
use tokio::task::spawn_blocking;
use tokio_util::task::AbortOnDropHandle;
use tracing::Instrument;

use super::camera::{self, OwnCamera};
use super::connection::{
    self, Connection, Dialled, OwnIndex, Unheard, Wiring, hear_as_chosen, use_own_index,
};
use super::devices::{Listing, select_microphone, select_speaker};
use super::feeds::Feeds;
use super::screen::{self, OwnScreen};
use super::{e2ee, stats};
use crate::adapters::video::camera::CameraDevice;
use crate::domain::call::device_test::{amplitude_loudness, loudness_percent};
use crate::domain::call::devices::{
    ByKind, DeviceChoice, DeviceChoices, DeviceKey, DeviceKind, DeviceLists, MediaDevice,
};
use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::keys::{KeyIndex, MediaKey};
use crate::domain::call::media::{MediaEvent, Microphone, SfuState};
use crate::domain::call::sfu::{SfuGrant, SfuId, SfuRole};
use crate::domain::call::video::{Camera, ScreenShare};
use crate::ports::call::{ConnectedSfu, MediaSession};

const MICROPHONE_TRACK: &str = "microphone";

struct Dialling {
    role: SfuRole,
    handle: AbortOnDropHandle<Result<Dialled, String>>,
}

struct Finished {
    sfu: SfuId,
    role: SfuRole,
    dialled: Result<Dialled, String>,
}

pub(super) struct LiveKitSession {
    audio: PlatformAudio,
    keys: Option<KeyProvider>,
    own_keys: Vec<(KeyIndex, MediaKey)>,
    own_index: OwnIndex,
    unheard: Unheard,
    dialling: HashMap<SfuId, Dialling>,
    connections: HashMap<SfuId, Connection>,
    microphone: Option<LocalAudioTrack>,
    wants_microphone: bool,
    camera: Option<OwnCamera>,
    wants_camera: bool,
    screen: Option<OwnScreen>,
    choices: DeviceChoices,
    failed: ByKind<Option<DeviceKey>>,
    listing: Listing,
    reported: Option<DeviceLists>,
    routed: ByKind<Option<u16>>,
    feeds: Arc<Feeds>,
    events_tx: mpsc::UnboundedSender<MediaEvent>,
}

impl LiveKitSession {
    pub(super) fn new(
        audio: PlatformAudio,
        encrypted: bool,
        choices: DeviceChoices,
        feeds: Arc<Feeds>,
        events_tx: mpsc::UnboundedSender<MediaEvent>,
    ) -> Self {
        Self {
            audio,
            keys: encrypted.then(e2ee::key_provider),
            own_keys: Vec::new(),
            own_index: OwnIndex::default(),
            unheard: Unheard::default(),
            dialling: HashMap::new(),
            connections: HashMap::new(),
            microphone: None,
            wants_microphone: false,
            camera: None,
            wants_camera: false,
            screen: None,
            choices,
            failed: ByKind::default(),
            listing: Listing::default(),
            reported: None,
            routed: ByKind::default(),
            feeds,
            events_tx,
        }
    }

    fn emit(&self, event: MediaEvent) {
        if self.events_tx.send(event).is_err() {
            tracing::debug!("the app stopped listening to the call media");
        }
    }

    fn publishing(&self) -> Option<&Connection> {
        self.connections
            .values()
            .find(|connection| connection.role == SfuRole::Publish)
    }

    fn finished_dial(&mut self, cx: &mut Context<'_>) -> Poll<Finished> {
        let ready = self.dialling.iter_mut().find_map(|(sfu, dialling)| {
            match Pin::new(&mut dialling.handle).poll(cx) {
                Poll::Ready(joined) => Some((sfu.clone(), joined)),
                Poll::Pending => None,
            }
        });
        let Some((sfu, joined)) = ready else {
            return Poll::Pending;
        };
        let Some(dialling) = self.dialling.remove(&sfu) else {
            return Poll::Pending;
        };
        let dialled =
            joined.unwrap_or_else(|e| Err(format!("the connection attempt stopped: {e}")));
        Poll::Ready(Finished {
            sfu,
            role: dialling.role,
            dialled,
        })
    }

    fn adopt(&mut self, sfu: SfuId, role: SfuRole, dialled: Dialled) -> ConnectedSfu {
        let connection = dialled.start(Wiring {
            sfu: sfu.clone(),
            role,
            own_index: &self.own_index,
            unheard: &self.unheard,
            feeds: &self.feeds,
            events_tx: &self.events_tx,
            encrypted: self.keys.is_some(),
        });
        self.install_own_keys(&connection);
        self.connections.insert(sfu.clone(), connection);
        ConnectedSfu { sfu, role }
    }

    fn dial_failed(&self, sfu: SfuId, why: String) {
        tracing::warn!(%sfu, "could not connect to the call media server: {why}");
        self.emit(MediaEvent::Sfu {
            sfu,
            state: SfuState::Disconnected(why),
        });
    }

    fn install_own_keys(&self, connection: &Connection) {
        let Some(provider) = &self.keys else {
            return;
        };
        let local = connection.room.local_participant().identity();
        for (index, key) in &self.own_keys {
            e2ee::install(provider, &local, *index, key);
        }
    }

    async fn publish_microphone(&mut self) {
        if self.microphone.is_some() || self.publishing().is_none() {
            return;
        }
        self.read_devices().await;
        self.route(DeviceKind::Microphone).await;
        let Some(connection) = self.publishing() else {
            return;
        };
        let room = Arc::clone(&connection.room);
        let track = LocalAudioTrack::create_audio_track(MICROPHONE_TRACK, self.audio.rtc_source());
        let options = TrackPublishOptions {
            source: TrackSource::Microphone,
            dtx: true,
            red: false,
            ..TrackPublishOptions::default()
        };
        match room
            .local_participant()
            .publish_track(LocalTrack::Audio(track.clone()), options)
            .await
        {
            Ok(_) => {
                tracing::info!("published the microphone");
                if let Some(index) = self.own_index.lock().ok().and_then(|index| *index) {
                    use_own_index(&room, index);
                }
                self.microphone = Some(track);
                self.emit(MediaEvent::Microphone(Microphone::On));
            }
            Err(e) => {
                tracing::warn!("the microphone could not be published: {e}");
                self.emit(MediaEvent::Microphone(Microphone::Failed(e.to_string())));
            }
        }
    }

    async fn start_camera(&mut self) {
        if self.camera.as_ref().is_some_and(OwnCamera::is_capturing) {
            return;
        }
        self.stop_camera().await;
        let Some(room) = self
            .publishing()
            .map(|connection| Arc::clone(&connection.room))
        else {
            return;
        };
        self.read_devices().await;
        let Some((chosen, device)) = self.camera_to_start() else {
            tracing::warn!("the camera did not start: no camera was found");
            self.wants_camera = false;
            self.emit(MediaEvent::Camera(Camera::Failed(
                "no camera was found".into(),
            )));
            return;
        };
        match camera::start(&room, device, &self.feeds, &self.events_tx).await {
            Ok(own) => {
                self.camera = Some(own);
                self.emit(MediaEvent::Camera(Camera::On(camera::own_feed())));
            }
            Err(why) => {
                tracing::warn!("the camera did not start: {why}");
                self.wants_camera = false;
                if self.choices.camera.key() == Some(&chosen.key) {
                    self.device_failed(DeviceKind::Camera, chosen.key, why.clone());
                }
                self.emit(MediaEvent::Camera(Camera::Failed(why)));
            }
        }
    }

    fn camera_to_start(&self) -> Option<(MediaDevice, CameraDevice)> {
        let chosen = self
            .choices
            .camera
            .resolve(
                DeviceKind::Camera,
                &self.listing.lists().connected.camera,
                self.failed.camera.as_ref(),
            )?
            .clone();
        let device = self.listing.camera(&chosen)?;
        Some((chosen, device))
    }

    async fn follow_camera(&mut self) {
        let DeviceChoice::Device(chosen) = self.choices.camera.clone() else {
            return;
        };
        if self.failed.camera.as_ref() == Some(&chosen.key) {
            return;
        }
        let Some(device) = self.listing.camera(&chosen) else {
            return;
        };
        let Some(own) = self.camera.as_mut().filter(|own| own.is_capturing()) else {
            return;
        };
        if own.device_key() == device.key {
            return;
        }
        let switched = own.switch_to(device, &self.feeds, &self.events_tx).await;
        let capturing = own.is_capturing();
        let Err(why) = switched else {
            tracing::info!(device = chosen.name, "the camera uses the one you chose");
            return;
        };
        tracing::warn!(
            device = chosen.name,
            "the camera you chose did not start: {why}"
        );
        self.device_failed(DeviceKind::Camera, chosen.key, why.clone());
        if !capturing {
            self.emit(MediaEvent::Camera(Camera::Failed(why)));
        }
    }

    async fn read_devices(&mut self) {
        let audio = self.audio.clone();
        let listing = match spawn_blocking(move || Listing::read(&audio)).await {
            Ok(listing) => listing,
            Err(e) => {
                tracing::debug!("the call devices could not be listed: {e}");
                return;
            }
        };
        self.listing = listing;
        let lists = self.listing.lists();
        for kind in DeviceKind::ALL {
            let failed = self.failed.of_mut(kind);
            if failed
                .as_ref()
                .is_some_and(|key| !lists.is_connected(kind, key))
            {
                *failed = None;
            }
        }
        if self.reported.as_ref() == Some(lists) {
            return;
        }
        let names = |devices: &[MediaDevice]| -> Vec<String> {
            devices.iter().map(|device| device.name.clone()).collect()
        };
        tracing::info!(
            microphones = ?names(&lists.connected.microphone),
            speakers = ?names(&lists.connected.speaker),
            cameras = ?names(&lists.connected.camera),
            default_microphone = ?lists.defaults.microphone,
            default_speaker = ?lists.defaults.speaker,
            "the call devices changed"
        );
        self.reported = Some(lists.clone());
        self.emit(MediaEvent::Devices(lists.clone()));
    }

    async fn route(&mut self, kind: DeviceKind) {
        loop {
            let chosen = self
                .choices
                .of(kind)
                .resolve(
                    kind,
                    self.listing.lists().connected.of(kind),
                    self.failed.of(kind).as_ref(),
                )
                .cloned();
            let index = self.listing.index_of(kind, chosen.as_ref());
            if *self.routed.of(kind) == Some(index) {
                return;
            }
            let live = kind == DeviceKind::Microphone && self.microphone_live();
            let audio = self.audio.clone();
            let selected = spawn_blocking(move || match kind {
                DeviceKind::Microphone => select_microphone(&audio, index, live),
                DeviceKind::Speaker => select_speaker(&audio, index),
                DeviceKind::Camera => Ok(()),
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            *self.routed.of_mut(kind) = Some(index);
            let name = chosen.as_ref().map(|device| device.name.as_str());
            let Err(why) = selected else {
                tracing::info!(%kind, device = name, live, "the call uses this device");
                return;
            };
            let Some(device) = chosen else {
                tracing::warn!(%kind, live, "the system default device did not start: {why}");
                if live {
                    self.emit(MediaEvent::Microphone(Microphone::Failed(why)));
                }
                return;
            };
            tracing::warn!(%kind, device = device.name, live, "a call device could not be used: {why}");
            self.device_failed(kind, device.key, why);
        }
    }

    fn microphone_live(&self) -> bool {
        self.wants_microphone && self.microphone.is_some()
    }

    async fn close_muted_microphone(&self) {
        if self.wants_microphone || self.microphone.is_none() {
            return;
        }
        let audio = self.audio.clone();
        let closed = spawn_blocking(move || {
            if !audio.is_recording_initialized() {
                return Ok(false);
            }
            audio.stop_recording().map(|()| true)
        })
        .await;
        match closed {
            Ok(Ok(true)) => {
                tracing::info!("closed the muted microphone, which had started recording again");
            }
            Ok(Err(e)) => tracing::debug!("the muted microphone did not close: {e}"),
            Ok(Ok(false)) | Err(_) => {}
        }
    }

    fn device_failed(&mut self, kind: DeviceKind, device: DeviceKey, why: String) {
        *self.failed.of_mut(kind) = Some(device.clone());
        self.emit(MediaEvent::DeviceFailed { kind, device, why });
    }

    async fn stop_camera(&mut self) {
        if let Some(own) = self.camera.take() {
            let room = self
                .publishing()
                .map(|connection| Arc::clone(&connection.room));
            own.stop(room.as_deref()).await;
        }
    }

    fn start_screen(&mut self) {
        if self.screen.as_ref().is_some_and(OwnScreen::is_running) {
            return;
        }
        let Some(room) = self
            .publishing()
            .map(|connection| Arc::clone(&connection.room))
        else {
            tracing::info!("the screen share waits for the call to connect");
            self.emit(MediaEvent::Screen(ScreenShare::Failed(
                "the call is not connected yet".into(),
            )));
            return;
        };
        self.screen = Some(screen::start(
            room,
            Arc::clone(&self.feeds),
            self.events_tx.clone(),
        ));
    }

    async fn stop_screen(&mut self) {
        if let Some(screen) = self.screen.take() {
            screen.stop().await;
        }
    }

    async fn switch_microphone(&mut self, on: bool) {
        if on {
            self.read_devices().await;
            self.route(DeviceKind::Microphone).await;
        }
        let Some(track) = &self.microphone else {
            return;
        };
        let switched = if on {
            self.audio.start_recording().map(|()| track.unmute())
        } else {
            track.mute();
            self.audio.stop_recording()
        };
        match switched {
            Ok(()) => {
                tracing::debug!(on, "switched the microphone");
                self.emit(MediaEvent::Microphone(if on {
                    Microphone::On
                } else {
                    Microphone::Off
                }));
            }
            Err(e) => {
                tracing::warn!(on, "the microphone did not switch: {e}");
                self.emit(MediaEvent::Microphone(Microphone::Failed(e.to_string())));
            }
        }
    }
}

#[async_trait]
impl MediaSession for LiveKitSession {
    fn dial(&mut self, grant: SfuGrant) {
        let sfu = grant.sfu.clone();
        if self.connections.contains_key(&sfu) || self.dialling.contains_key(&sfu) {
            tracing::debug!(%sfu, "already connected or connecting to this call media server");
            return;
        }
        self.emit(MediaEvent::Sfu {
            sfu: sfu.clone(),
            state: SfuState::Connecting,
        });
        let role = grant.role;
        let handle = tokio::spawn(connection::dial(grant, self.keys.clone()).in_current_span());
        self.dialling.insert(
            sfu,
            Dialling {
                role,
                handle: AbortOnDropHandle::new(handle),
            },
        );
    }

    async fn next_connected(&mut self) -> ConnectedSfu {
        loop {
            let finished = poll_fn(|cx| self.finished_dial(cx)).await;
            match finished.dialled {
                Ok(dialled) => return self.adopt(finished.sfu, finished.role, dialled),
                Err(why) => self.dial_failed(finished.sfu, why),
            }
        }
    }

    async fn disconnect(&mut self, sfu: &SfuId) {
        if let Some(dialling) = self.dialling.remove(sfu) {
            abandon(sfu, dialling).await;
        }
        if let Some(connection) = self.connections.remove(sfu) {
            connection.close(sfu).await;
        }
        self.feeds.forget_server(sfu);
    }

    async fn set_microphone(&mut self, on: bool) {
        self.wants_microphone = on;
        if self.microphone.is_some() {
            self.switch_microphone(on).await;
        } else if on {
            self.publish_microphone().await;
        }
    }

    async fn set_camera(&mut self, on: bool) {
        self.wants_camera = on;
        if on {
            self.failed.camera = None;
            self.start_camera().await;
        } else {
            self.stop_camera().await;
        }
    }

    async fn use_device(&mut self, kind: DeviceKind, choice: DeviceChoice) {
        *self.choices.of_mut(kind) = choice;
        *self.failed.of_mut(kind) = None;
        self.read_devices().await;
        match kind {
            DeviceKind::Camera => self.follow_camera().await,
            DeviceKind::Microphone | DeviceKind::Speaker => self.route(kind).await,
        }
    }

    async fn refresh_devices(&mut self) {
        self.read_devices().await;
        self.route(DeviceKind::Microphone).await;
        self.route(DeviceKind::Speaker).await;
        self.follow_camera().await;
        self.close_muted_microphone().await;
    }

    async fn set_screen_share(&mut self, on: bool) {
        if on {
            self.start_screen();
        } else {
            self.stop_screen().await;
        }
    }

    async fn set_own_key(&mut self, index: KeyIndex, key: &MediaKey) {
        self.own_keys.retain(|(known, _)| *known != index);
        self.own_keys.push((index, key.clone()));
        let Some(provider) = &self.keys else {
            return;
        };
        for connection in self.connections.values() {
            let local = connection.room.local_participant().identity();
            e2ee::install(provider, &local, index, key);
        }
        tracing::debug!(%index, "installed your call key");
    }

    async fn use_own_key(&mut self, index: KeyIndex) {
        if let Ok(mut current) = self.own_index.lock() {
            *current = Some(index);
        }
        for connection in self.connections.values() {
            use_own_index(&connection.room, index);
        }
        tracing::debug!(%index, "encrypting your call media with a new key");
    }

    async fn set_key(&mut self, identities: &[MediaIdentity], index: KeyIndex, key: &MediaKey) {
        let Some(provider) = &self.keys else {
            return;
        };
        for identity in identities {
            let identity = ParticipantIdentity(identity.as_ref().to_owned());
            e2ee::install(provider, &identity, index, key);
        }
    }

    async fn sample_microphone(&mut self) {
        let Some(track) = self.microphone.as_ref().filter(|_| self.wants_microphone) else {
            return;
        };
        let stats = match track.get_stats().await {
            Ok(stats) => stats,
            Err(e) => {
                tracing::debug!("your microphone's level could not be read: {e}");
                return;
            }
        };
        let amplitude = stats.iter().find_map(|stats| match stats {
            RtcStats::MediaSource(source) if source.source.kind == "audio" => {
                Some(source.audio.audio_level)
            }
            _ => None,
        });
        if let Some(amplitude) = amplitude {
            self.emit(MediaEvent::MicrophoneLevel(loudness_percent(
                amplitude_loudness(amplitude),
            )));
        }
    }

    async fn sample_connections(&mut self) {
        let mut readings = Vec::new();
        for (sfu, connection) in &self.connections {
            if let Some(reading) = stats::reading(sfu, &connection.room).await {
                readings.push(reading);
            }
        }
        self.emit(MediaEvent::Connections(readings));
    }

    async fn set_unheard(&mut self, identities: &[MediaIdentity]) {
        if let Ok(mut unheard) = self.unheard.lock() {
            *unheard = identities.iter().cloned().collect();
        }
        for (sfu, connection) in &self.connections {
            hear_as_chosen(&connection.room, &self.unheard, sfu);
        }
    }

    async fn close(mut self: Box<Self>) {
        let dialling: Vec<(SfuId, Dialling)> = self.dialling.drain().collect();
        for (_, abandoned) in &dialling {
            abandoned.handle.abort();
        }
        if let Some(track) = self.microphone.take() {
            track.mute();
        }
        if let Err(e) = self.audio.stop_recording() {
            tracing::debug!("the microphone did not stop: {e}");
        }
        if let Some(screen) = self.screen.take() {
            screen.stop().await;
        }
        if let Some(own) = self.camera.take() {
            own.stop(None).await;
        }
        for (sfu, abandoned) in dialling {
            abandon(&sfu, abandoned).await;
        }
        let connections: Vec<(SfuId, Connection)> = self.connections.drain().collect();
        for (sfu, connection) in connections {
            connection.close(&sfu).await;
        }
        self.feeds.clear();
    }
}

async fn abandon(sfu: &SfuId, dialling: Dialling) {
    dialling.handle.abort();
    if let Ok(Ok(dialled)) = dialling.handle.await {
        tracing::debug!(%sfu, "closing a call media server connection nobody wants any more");
        dialled.close(sfu).await;
    }
}

impl Drop for LiveKitSession {
    fn drop(&mut self) {
        if !self.connections.is_empty() {
            tracing::warn!(
                connections = self.connections.len(),
                "the call media was dropped without closing its connections"
            );
        }
    }
}
