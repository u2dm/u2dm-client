use std::sync::{Arc, Mutex};
use std::time::Duration;

use livekit::e2ee::EncryptionType;
use livekit::e2ee::key_provider::KeyProvider;
use livekit::prelude::{
    ConnectionState, Participant, RemoteParticipant, RemoteTrack, RemoteTrackPublication,
    RemoteVideoTrack, Room, RoomEvent, RoomOptions, TrackPublication, TrackSource,
};
use livekit::webrtc::native::frame_cryptor::EncryptionState;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::Instrument;

use super::e2ee;
use super::feeds::{Feeds, feed_of};
use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::keys::KeyIndex;
use crate::domain::call::media::{Decryption, MediaEvent, SfuState};
use crate::domain::call::sfu::{SfuGrant, SfuId, SfuRole};
use crate::domain::call::video::VideoKind;

const CONNECT_WAIT: Duration = Duration::from_secs(15);
pub(super) const MEDIA_CLOSE_WAIT: Duration = Duration::from_secs(1);

pub(super) type OwnIndex = Arc<Mutex<Option<KeyIndex>>>;

pub(super) struct Connection {
    pub(super) room: Arc<Room>,
    pub(super) role: SfuRole,
    pump: JoinHandle<()>,
}

pub(super) struct Dial<'a> {
    pub(super) grant: &'a SfuGrant,
    pub(super) encryption: Option<&'a KeyProvider>,
    pub(super) own_index: &'a OwnIndex,
    pub(super) feeds: &'a Arc<Feeds>,
    pub(super) events_tx: &'a mpsc::UnboundedSender<MediaEvent>,
}

pub(super) async fn dial(dial: Dial<'_>) -> Result<Connection, String> {
    let sfu = dial.grant.sfu.clone();
    let mut options = RoomOptions::default();
    options.auto_subscribe = true;
    options.dynacast = true;
    options.encryption = dial.encryption.map(e2ee::options);
    let host = super::host_of(&dial.grant.url).to_owned();
    tracing::info!(%sfu, host, role = ?dial.grant.role, encrypted = dial.encryption.is_some(), "connecting to a call media server");
    let connected = timeout(
        CONNECT_WAIT,
        Room::connect(&dial.grant.url, dial.grant.token.expose(), options),
    )
    .await;
    let (room, events) = match connected {
        Ok(Ok(connected)) => connected,
        Ok(Err(e)) => return Err(e.to_string()),
        Err(_) => return Err("the media server did not answer in time".into()),
    };
    let room = Arc::new(room);
    let local = room.local_participant().identity();
    tracing::info!(%sfu, identity = %local, "connected to a call media server");
    let pump = tokio::spawn(
        pump_events(Pump {
            sfu,
            room: Arc::clone(&room),
            events,
            events_tx: dial.events_tx.clone(),
            own_index: Arc::clone(dial.own_index),
            feeds: Arc::clone(dial.feeds),
            encrypted: dial.encryption.is_some(),
        })
        .in_current_span(),
    );
    Ok(Connection {
        room,
        role: dial.grant.role,
        pump,
    })
}

impl Connection {
    pub(super) async fn close(self, sfu: &SfuId) {
        match timeout(MEDIA_CLOSE_WAIT, self.room.close()).await {
            Ok(Ok(())) => tracing::debug!(%sfu, "left the call media server"),
            Ok(Err(e)) => {
                tracing::debug!(%sfu, "the call media server connection did not close cleanly: {e}");
            }
            Err(_) => tracing::info!(%sfu, "the call media server did not close in time"),
        }
        self.pump.abort();
    }
}

pub(super) fn use_own_index(room: &Room, index: KeyIndex) {
    let local = room.local_participant().identity();
    for ((identity, _), cryptor) in room.e2ee_manager().frame_cryptors() {
        if identity == local {
            cryptor.set_key_index(i32::from(index.get()));
        }
    }
}

struct Pump {
    sfu: SfuId,
    room: Arc<Room>,
    events: mpsc::UnboundedReceiver<RoomEvent>,
    events_tx: mpsc::UnboundedSender<MediaEvent>,
    own_index: OwnIndex,
    feeds: Arc<Feeds>,
    encrypted: bool,
}

async fn pump_events(mut pump: Pump) {
    pump.emit(MediaEvent::Sfu {
        sfu: pump.sfu.clone(),
        state: SfuState::Connected,
    });
    while let Some(event) = pump.events.recv().await {
        pump.handle(event);
    }
    tracing::debug!(sfu = %pump.sfu, "the call media server events ended");
}

impl Pump {
    fn emit(&self, event: MediaEvent) {
        if self.events_tx.send(event).is_err() {
            tracing::debug!(sfu = %self.sfu, "the call media session is gone");
        }
    }

    fn handle(&self, event: RoomEvent) {
        match event {
            RoomEvent::Connected {
                participants_with_tracks,
            } => {
                for (participant, publications) in participants_with_tracks {
                    self.present(&participant, true);
                    for publication in publications {
                        self.publication(&participant, &publication);
                    }
                }
            }
            RoomEvent::ParticipantConnected(participant) => self.present(&participant, true),
            RoomEvent::ParticipantDisconnected(participant) => self.present(&participant, false),
            RoomEvent::TrackSubscribed {
                track,
                publication,
                participant,
            } => {
                if self.publication(&participant, &publication)
                    && let RemoteTrack::Video(video) = track
                {
                    self.video_subscribed(&participant, video, publication);
                }
            }
            RoomEvent::TrackUnsubscribed {
                publication,
                participant,
                ..
            }
            | RoomEvent::TrackUnpublished {
                publication,
                participant,
            } => self.track_gone(&participant, publication.source()),
            RoomEvent::TrackSubscriptionFailed {
                participant, error, ..
            } => {
                tracing::info!(sfu = %self.sfu, identity = %participant.identity(), "could not receive a call participant's media: {error}");
            }
            RoomEvent::TrackMuted {
                participant,
                publication,
            } => self.muted(&participant, &publication, true),
            RoomEvent::TrackUnmuted {
                participant,
                publication,
            } => self.muted(&participant, &publication, false),
            RoomEvent::ActiveSpeakersChanged { speakers } => self.emit(MediaEvent::Speaking {
                sfu: self.sfu.clone(),
                speakers: speakers
                    .iter()
                    .map(|speaker| MediaIdentity::new(speaker.identity().to_string()))
                    .collect(),
            }),
            RoomEvent::E2eeStateChanged { participant, state } => {
                self.encryption_state(&participant, state);
            }
            RoomEvent::ConnectionStateChanged(state) => self.connection_state(state),
            RoomEvent::Reconnecting => self.sfu_state(SfuState::Reconnecting),
            RoomEvent::Reconnected => self.sfu_state(SfuState::Connected),
            RoomEvent::Disconnected { reason } => {
                self.sfu_state(SfuState::Disconnected(format!("{reason:?}")));
            }
            RoomEvent::LocalTrackPublished { .. } | RoomEvent::LocalTrackRepublished { .. } => {
                self.reapply_own_index();
            }
            _ => {}
        }
    }

    fn sfu_state(&self, state: SfuState) {
        self.emit(MediaEvent::Sfu {
            sfu: self.sfu.clone(),
            state,
        });
    }

    fn connection_state(&self, state: ConnectionState) {
        let state = match state {
            ConnectionState::Connected => SfuState::Connected,
            ConnectionState::Reconnecting => SfuState::Reconnecting,
            ConnectionState::Disconnected => return,
        };
        self.sfu_state(state);
    }

    fn present(&self, participant: &RemoteParticipant, present: bool) {
        tracing::debug!(sfu = %self.sfu, identity = %participant.identity(), present, "a call participant changed");
        self.emit(MediaEvent::Present {
            sfu: self.sfu.clone(),
            identity: identity_of(participant),
            present,
        });
        if present {
            self.unheard(participant);
        }
    }

    fn unheard(&self, participant: &RemoteParticipant) {
        self.emit(MediaEvent::Muted {
            sfu: self.sfu.clone(),
            identity: identity_of(participant),
            muted: true,
        });
    }

    fn publication(
        &self,
        participant: &RemoteParticipant,
        publication: &RemoteTrackPublication,
    ) -> bool {
        let identity = identity_of(participant);
        if self.encrypted && publication.encryption_type() == EncryptionType::None {
            tracing::warn!(sfu = %self.sfu, %identity, "refused unencrypted media in an encrypted call");
            publication.set_subscribed(false);
            self.emit(MediaEvent::Decryption {
                identity,
                state: Decryption::Failed,
            });
            return false;
        }
        if publication.source() == TrackSource::Microphone {
            self.emit(MediaEvent::Muted {
                sfu: self.sfu.clone(),
                identity,
                muted: publication.is_muted(),
            });
        }
        true
    }

    fn video_subscribed(
        &self,
        participant: &RemoteParticipant,
        track: RemoteVideoTrack,
        publication: RemoteTrackPublication,
    ) {
        let Some(kind) = video_kind(publication.source()) else {
            return;
        };
        let identity = identity_of(participant);
        let feed = feed_of(&self.sfu, &identity, kind);
        tracing::debug!(sfu = %self.sfu, %identity, ?kind, muted = publication.is_muted(), "a call participant's video arrived");
        let muted = publication.is_muted();
        self.feeds.add_remote(&feed, track, publication);
        self.emit(MediaEvent::Video {
            sfu: self.sfu.clone(),
            identity,
            kind,
            feed: (!muted).then_some(feed),
        });
    }

    fn track_gone(&self, participant: &RemoteParticipant, source: TrackSource) {
        if source == TrackSource::Microphone {
            self.unheard(participant);
            return;
        }
        let Some(kind) = video_kind(source) else {
            return;
        };
        let identity = identity_of(participant);
        tracing::debug!(sfu = %self.sfu, %identity, ?kind, "a call participant's video ended");
        self.feeds
            .remove_remote(&feed_of(&self.sfu, &identity, kind));
        self.emit(MediaEvent::Video {
            sfu: self.sfu.clone(),
            identity,
            kind,
            feed: None,
        });
    }

    fn muted(&self, participant: &Participant, publication: &TrackPublication, muted: bool) {
        if matches!(participant, Participant::Local(_)) {
            return;
        }
        let identity = MediaIdentity::new(participant.identity().to_string());
        let source = publication.source();
        if source == TrackSource::Microphone {
            self.emit(MediaEvent::Muted {
                sfu: self.sfu.clone(),
                identity,
                muted,
            });
            return;
        }
        let Some(kind) = video_kind(source) else {
            return;
        };
        let feed = feed_of(&self.sfu, &identity, kind);
        self.emit(MediaEvent::Video {
            sfu: self.sfu.clone(),
            identity,
            kind,
            feed: (!muted).then_some(feed),
        });
    }

    fn encryption_state(&self, participant: &Participant, state: EncryptionState) {
        let identity = MediaIdentity::new(participant.identity().to_string());
        let decryption = match state {
            EncryptionState::Ok | EncryptionState::KeyRatcheted => Decryption::Ok,
            EncryptionState::MissingKey => Decryption::MissingKey,
            EncryptionState::DecryptionFailed | EncryptionState::InternalError => {
                Decryption::Failed
            }
            EncryptionState::EncryptionFailed => {
                tracing::warn!(sfu = %self.sfu, %identity, "your call media failed to encrypt");
                return;
            }
            EncryptionState::New => return,
        };
        if matches!(participant, Participant::Local(_)) {
            return;
        }
        tracing::info!(sfu = %self.sfu, %identity, ?state, "a call participant's decryption changed");
        self.emit(MediaEvent::Decryption {
            identity,
            state: decryption,
        });
    }

    fn reapply_own_index(&self) {
        let index = self.own_index.lock().ok().and_then(|index| *index);
        if let Some(index) = index {
            use_own_index(&self.room, index);
        }
    }
}

const fn video_kind(source: TrackSource) -> Option<VideoKind> {
    match source {
        TrackSource::Camera => Some(VideoKind::Camera),
        TrackSource::Screenshare => Some(VideoKind::Screen),
        TrackSource::Unknown | TrackSource::Microphone | TrackSource::ScreenshareAudio => None,
    }
}

fn identity_of(participant: &RemoteParticipant) -> MediaIdentity {
    MediaIdentity::new(participant.identity().to_string())
}
