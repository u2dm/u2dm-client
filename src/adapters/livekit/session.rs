use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use livekit::e2ee::key_provider::KeyProvider;
use livekit::id::ParticipantIdentity;
use livekit::options::TrackPublishOptions;
use livekit::prelude::{LocalAudioTrack, LocalTrack, PlatformAudio, TrackSource};
use tokio::sync::mpsc;

use super::camera::{self, OwnCamera};
use super::connection::{Connection, Dial, OwnIndex, dial, use_own_index};
use super::e2ee;
use super::feeds::Feeds;
use super::screen::{self, OwnScreen};
use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::keys::{KeyIndex, MediaKey};
use crate::domain::call::media::{MediaEvent, Microphone, SfuState};
use crate::domain::call::sfu::{SfuGrant, SfuId, SfuRole};
use crate::domain::call::video::{Camera, ScreenShare};
use crate::error::{AppError, Result};
use crate::ports::call::MediaSession;

const MICROPHONE_TRACK: &str = "microphone";

pub(super) struct LiveKitSession {
    audio: PlatformAudio,
    keys: Option<KeyProvider>,
    own_keys: Vec<(KeyIndex, MediaKey)>,
    own_index: OwnIndex,
    connections: HashMap<SfuId, Connection>,
    microphone: Option<LocalAudioTrack>,
    wants_microphone: bool,
    camera: Option<OwnCamera>,
    wants_camera: bool,
    screen: Option<OwnScreen>,
    feeds: Arc<Feeds>,
    events_tx: mpsc::UnboundedSender<MediaEvent>,
}

impl LiveKitSession {
    pub(super) fn new(
        audio: PlatformAudio,
        encrypted: bool,
        feeds: Arc<Feeds>,
        events_tx: mpsc::UnboundedSender<MediaEvent>,
    ) -> Self {
        Self {
            audio,
            keys: encrypted.then(e2ee::key_provider),
            own_keys: Vec::new(),
            own_index: OwnIndex::default(),
            connections: HashMap::new(),
            microphone: None,
            wants_microphone: false,
            camera: None,
            wants_camera: false,
            screen: None,
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
        if self.microphone.is_some() {
            return;
        }
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
        if self.camera.is_some() {
            return;
        }
        let Some(room) = self
            .publishing()
            .map(|connection| Arc::clone(&connection.room))
        else {
            return;
        };
        match camera::start(&room, &self.feeds, &self.events_tx).await {
            Ok(own) => {
                self.camera = Some(own);
                self.emit(MediaEvent::Camera(Camera::On(camera::own_feed())));
            }
            Err(why) => {
                tracing::warn!("the camera did not start: {why}");
                self.wants_camera = false;
                self.emit(MediaEvent::Camera(Camera::Failed(why)));
            }
        }
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
        self.screen = Some(screen::start(room, self.events_tx.clone()));
    }

    async fn stop_screen(&mut self) {
        if let Some(screen) = self.screen.take() {
            screen.stop().await;
        }
    }

    fn switch_microphone(&self, on: bool) {
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
    async fn connect(&mut self, grant: SfuGrant) -> Result<()> {
        let sfu = grant.sfu.clone();
        if self.connections.contains_key(&sfu) {
            return Ok(());
        }
        self.emit(MediaEvent::Sfu {
            sfu: sfu.clone(),
            state: SfuState::Connecting,
        });
        let dialled = dial(Dial {
            grant: &grant,
            encryption: self.keys.as_ref(),
            own_index: &self.own_index,
            feeds: &self.feeds,
            events_tx: &self.events_tx,
        })
        .await;
        let connection = match dialled {
            Ok(connection) => connection,
            Err(why) => {
                tracing::warn!(%sfu, "could not connect to the call media server: {why}");
                self.emit(MediaEvent::Sfu {
                    sfu,
                    state: SfuState::Disconnected(why.clone()),
                });
                return Err(AppError::Other(why));
            }
        };
        self.install_own_keys(&connection);
        let publishes = connection.role == SfuRole::Publish;
        self.connections.insert(sfu, connection);
        if publishes && self.wants_microphone {
            self.publish_microphone().await;
        }
        if publishes && self.wants_camera {
            self.start_camera().await;
        }
        Ok(())
    }

    async fn disconnect(&mut self, sfu: &SfuId) {
        if let Some(connection) = self.connections.remove(sfu) {
            connection.close(sfu).await;
        }
        self.feeds.forget_server(sfu);
    }

    async fn set_microphone(&mut self, on: bool) {
        self.wants_microphone = on;
        if self.microphone.is_some() {
            self.switch_microphone(on);
        } else if on {
            self.publish_microphone().await;
        }
    }

    async fn set_camera(&mut self, on: bool) {
        self.wants_camera = on;
        if on {
            self.start_camera().await;
        } else {
            self.stop_camera().await;
        }
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

    async fn close(mut self: Box<Self>) {
        if let Some(screen) = self.screen.take() {
            screen.stop().await;
        }
        if let Some(own) = self.camera.take() {
            own.stop(None).await;
        }
        let connections: Vec<(SfuId, Connection)> = self.connections.drain().collect();
        for (sfu, connection) in connections {
            connection.close(&sfu).await;
        }
        if let Some(track) = self.microphone.take() {
            track.mute();
        }
        if let Err(e) = self.audio.stop_recording() {
            tracing::debug!("the microphone did not stop: {e}");
        }
        self.feeds.clear();
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
