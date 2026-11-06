use std::future::poll_fn;

use async_trait::async_trait;
use livekit::id::ParticipantIdentity;
use livekit::webrtc::stats::RtcStats;
use tokio_util::task::AbortOnDropHandle;
use tracing::Instrument;

use super::{Dialling, LiveKitSession, abandon};
use crate::adapters::livekit::connection::{self, Connection, hear_as_chosen, use_own_index};
use crate::adapters::livekit::{e2ee, stats};
use crate::domain::call::device_test::{amplitude_loudness, loudness_percent};
use crate::domain::call::devices::{DeviceChoice, DeviceKind};
use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::keys::{KeyIndex, MediaKey};
use crate::domain::call::media::{MediaEvent, SfuState};
use crate::domain::call::sfu::{SfuGrant, SfuId};
use crate::ports::call::{ConnectedSfu, MediaSession};

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
