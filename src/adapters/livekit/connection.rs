use std::collections::HashSet;
use std::future::pending;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use livekit::e2ee::EncryptionType;
use livekit::e2ee::key_provider::KeyProvider;
use livekit::prelude::{
    ConnectionQuality as LiveKitQuality, ConnectionState, Participant, RemoteAudioTrack,
    RemoteParticipant, RemoteTrack, RemoteTrackPublication, RemoteVideoTrack, Room, RoomEvent,
    RoomOptions, TrackPublication, TrackSource,
};
use livekit::webrtc::native::frame_cryptor::EncryptionState;
use livekit::webrtc::stats::RtcStats;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior, interval_at, sleep_until, timeout};
use tracing::Instrument;

use super::e2ee;
use super::feeds::{Feeds, feed_of};
use super::quality::{Qualities, QualityOf, Rated};
use super::reception::{Change, Flow, Flowing, RECEIVE_CHECK, Reception, Wanted};
use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::keys::KeyIndex;
use crate::domain::call::media::{
    ConnectionQuality, Decryption, MediaEvent, MediaSource, SfuState,
};
use crate::domain::call::sfu::{SfuGrant, SfuId, SfuRole};
use crate::domain::call::video::VideoKind;

const CONNECT_WAIT: Duration = Duration::from_secs(15);
pub(super) const MEDIA_CLOSE_WAIT: Duration = Duration::from_secs(1);

pub(super) type OwnIndex = Arc<Mutex<Option<KeyIndex>>>;
pub(super) type Unheard = Arc<Mutex<HashSet<MediaIdentity>>>;

pub(super) struct Connection {
    pub(super) room: Arc<Room>,
    pub(super) role: SfuRole,
    pump: JoinHandle<()>,
}

pub(super) struct Dialled {
    room: Room,
    events: mpsc::UnboundedReceiver<RoomEvent>,
}

pub(super) struct Wiring<'a> {
    pub(super) sfu: SfuId,
    pub(super) role: SfuRole,
    pub(super) own_index: &'a OwnIndex,
    pub(super) unheard: &'a Unheard,
    pub(super) feeds: &'a Arc<Feeds>,
    pub(super) events_tx: &'a mpsc::UnboundedSender<MediaEvent>,
    pub(super) encrypted: bool,
}

pub(super) async fn dial(
    grant: SfuGrant,
    encryption: Option<KeyProvider>,
) -> Result<Dialled, String> {
    let mut options = RoomOptions::default();
    options.auto_subscribe = true;
    options.dynacast = true;
    options.encryption = encryption.as_ref().map(e2ee::options);
    let host = super::host_of(&grant.url).to_owned();
    tracing::info!(sfu = %grant.sfu, host, role = ?grant.role, encrypted = encryption.is_some(), "connecting to a call media server");
    let connected = timeout(
        CONNECT_WAIT,
        Room::connect(&grant.url, grant.token.expose(), options),
    )
    .await;
    let (room, events) = match connected {
        Ok(Ok(connected)) => connected,
        Ok(Err(e)) => return Err(e.to_string()),
        Err(_) => return Err("the media server did not answer in time".into()),
    };
    tracing::info!(sfu = %grant.sfu, identity = %room.local_participant().identity(), "connected to a call media server");
    Ok(Dialled { room, events })
}

impl Dialled {
    pub(super) fn start(self, wiring: Wiring<'_>) -> Connection {
        let room = Arc::new(self.room);
        let pump = tokio::spawn(
            pump_events(Pump {
                sfu: wiring.sfu,
                room: Arc::clone(&room),
                events: self.events,
                events_tx: wiring.events_tx.clone(),
                own_index: Arc::clone(wiring.own_index),
                unheard: Arc::clone(wiring.unheard),
                feeds: Arc::clone(wiring.feeds),
                encrypted: wiring.encrypted,
                reception: Reception::default(),
                flow: Flow::default(),
                qualities: Qualities::default(),
            })
            .in_current_span(),
        );
        Connection {
            room,
            role: wiring.role,
            pump,
        }
    }

    pub(super) async fn close(self, sfu: &SfuId) {
        close_room(&self.room, sfu).await;
    }
}

impl Connection {
    pub(super) async fn close(self, sfu: &SfuId) {
        close_room(&self.room, sfu).await;
        self.pump.abort();
    }
}

async fn close_room(room: &Room, sfu: &SfuId) {
    match timeout(MEDIA_CLOSE_WAIT, room.close()).await {
        Ok(Ok(())) => tracing::debug!(%sfu, "left the call media server"),
        Ok(Err(e)) => {
            tracing::debug!(%sfu, "the call media server connection did not close cleanly: {e}");
        }
        Err(_) => tracing::info!(%sfu, "the call media server did not close in time"),
    }
}

pub(super) fn hear_as_chosen(room: &Room, unheard: &Unheard, sfu: &SfuId) {
    for participant in room.remote_participants().into_values() {
        let identity = identity_of(&participant);
        let heard = is_heard(unheard, &identity);
        for publication in participant.track_publications().into_values() {
            if let Some(RemoteTrack::Audio(track)) = publication.track() {
                hear_or_not(&track, heard, sfu, &identity);
            }
        }
    }
}

fn is_heard(unheard: &Unheard, identity: &MediaIdentity) -> bool {
    unheard
        .lock()
        .map_or(true, |unheard| !unheard.contains(identity))
}

fn hear_or_not(track: &RemoteAudioTrack, heard: bool, sfu: &SfuId, identity: &MediaIdentity) {
    if track.is_enabled() == heard {
        return;
    }
    if heard {
        track.enable();
    } else {
        track.disable();
    }
    tracing::info!(%sfu, %identity, source = ?track.source(), heard, "changed whether you hear a call participant");
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
    unheard: Unheard,
    feeds: Arc<Feeds>,
    encrypted: bool,
    reception: Reception,
    flow: Flow,
    qualities: Qualities,
}

async fn pump_events(mut pump: Pump) {
    pump.emit(MediaEvent::Sfu {
        sfu: pump.sfu.clone(),
        state: SfuState::Connected,
    });
    let mut checks = interval_at(Instant::now() + RECEIVE_CHECK, RECEIVE_CHECK);
    checks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        let hold_ends = pump.qualities.next_due();
        tokio::select! {
            event = pump.events.recv() => match event {
                Some(event) => pump.handle(event),
                None => break,
            },
            _ = checks.tick() => {
                pump.check_reception();
                pump.check_flow().await;
            }
            () = until(hold_ends) => pump.hold_ended(),
        }
    }
    pump.ended();
}

async fn until(due: Option<Instant>) {
    match due {
        Some(at) => sleep_until(at).await,
        None => pending().await,
    }
}

impl Pump {
    fn speaking_now(&self) -> Vec<MediaIdentity> {
        let local = self.room.local_participant();
        let mut speaking: Vec<(f32, MediaIdentity)> = self
            .room
            .remote_participants()
            .values()
            .filter(|participant| participant.is_speaking())
            .map(|participant| {
                let identity = MediaIdentity::new(participant.identity().to_string());
                (participant.audio_level(), identity)
            })
            .collect();
        if local.is_speaking() {
            let identity = MediaIdentity::new(local.identity().to_string());
            speaking.push((local.audio_level(), identity));
        }
        speaking.sort_by(|one, other| other.0.total_cmp(&one.0));
        speaking.into_iter().map(|(_, identity)| identity).collect()
    }

    fn emit(&self, event: MediaEvent) {
        if self.events_tx.send(event).is_err() {
            tracing::debug!(sfu = %self.sfu, "the call media session is gone");
        }
    }

    fn ended(&self) {
        tracing::debug!(sfu = %self.sfu, "the call media server events ended");
    }

    fn handle(&mut self, event: RoomEvent) {
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
            RoomEvent::ParticipantDisconnected(participant) => self.departed(&participant),
            RoomEvent::TrackSubscribed {
                track,
                publication,
                participant,
            } => {
                if self.publication(&participant, &publication) {
                    match track {
                        RemoteTrack::Video(video) => {
                            self.video_subscribed(&participant, video, publication);
                        }
                        RemoteTrack::Audio(audio) => {
                            let identity = identity_of(&participant);
                            let heard = is_heard(&self.unheard, &identity);
                            hear_or_not(&audio, heard, &self.sfu, &identity);
                        }
                    }
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
                participant,
                error,
                track_sid,
            } => {
                tracing::info!(sfu = %self.sfu, identity = %participant.identity(), sid = %track_sid, "could not receive a call participant's media: {error}");
            }
            RoomEvent::TrackMuted {
                participant,
                publication,
            } => self.muted(&participant, &publication, true),
            RoomEvent::TrackUnmuted {
                participant,
                publication,
            } => self.muted(&participant, &publication, false),
            RoomEvent::ActiveSpeakersChanged { .. } => self.emit(MediaEvent::Speaking {
                sfu: self.sfu.clone(),
                speakers: self.speaking_now(),
            }),
            RoomEvent::E2eeStateChanged { participant, state } => {
                self.encryption_state(&participant, state);
            }
            RoomEvent::ConnectionQualityChanged {
                quality,
                participant,
            } => self.quality_changed(&participant, quality),
            RoomEvent::ConnectionStateChanged(state) => self.connection_state(state),
            RoomEvent::Reconnecting => self.reconnecting(),
            RoomEvent::Reconnected => {
                self.feeds.redeliver(&self.sfu);
                self.sfu_state(SfuState::Connected);
            }
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

    fn connection_state(&mut self, state: ConnectionState) {
        match state {
            ConnectionState::Connected => self.sfu_state(SfuState::Connected),
            ConnectionState::Reconnecting => self.reconnecting(),
            ConnectionState::Disconnected => {}
        }
    }

    fn reconnecting(&mut self) {
        self.reception.restart();
        for (identity, stalled) in self.flow.restart() {
            self.tell_stalled(identity, stalled);
        }
        self.sfu_state(SfuState::Reconnecting);
    }

    async fn check_flow(&mut self) {
        if self.room.connection_state() != ConnectionState::Connected {
            return;
        }
        let mut flowing = Vec::new();
        for participant in self.room.remote_participants().into_values() {
            let identity = identity_of(&participant);
            for publication in participant.track_publications().into_values() {
                if publication.source() != TrackSource::Camera || publication.is_muted() {
                    continue;
                }
                let Some(RemoteTrack::Video(track)) = publication.track() else {
                    continue;
                };
                if !track.is_enabled() {
                    continue;
                }
                if let Some(packets) = packets_received(&track).await {
                    flowing.push(Flowing {
                        sid: String::from(publication.sid()),
                        identity: identity.clone(),
                        packets,
                    });
                }
            }
        }
        for (identity, stalled) in self.flow.check(flowing) {
            self.tell_stalled(identity, stalled);
        }
    }

    fn tell_stalled(&self, identity: MediaIdentity, stalled: bool) {
        if stalled {
            tracing::warn!(sfu = %self.sfu, %identity, "a call participant's camera stopped arriving");
        } else {
            tracing::info!(sfu = %self.sfu, %identity, "a call participant's camera arrives again");
        }
        self.emit(MediaEvent::Stalled {
            sfu: self.sfu.clone(),
            identity,
            stalled,
        });
    }

    fn departed(&mut self, participant: &RemoteParticipant) {
        self.present(participant, false);
        if let Some(rated) = self
            .qualities
            .forget(&QualityOf::Member(identity_of(participant)))
        {
            self.tell_quality(rated);
        }
    }

    fn check_reception(&mut self) {
        if self.room.connection_state() != ConnectionState::Connected {
            return;
        }
        let wanted = self.wanted();
        for change in self.reception.check(Instant::now(), &wanted) {
            self.tell_missing(change);
        }
    }

    fn wanted(&self) -> Vec<Wanted> {
        let mut wanted = Vec::new();
        for participant in self.room.remote_participants().into_values() {
            let identity = identity_of(&participant);
            for publication in participant.track_publications().into_values() {
                let refused =
                    self.encrypted && publication.encryption_type() == EncryptionType::None;
                if publication.is_desired() && publication.track().is_none() && !refused {
                    wanted.push(Wanted {
                        sid: String::from(publication.sid()),
                        identity: identity.clone(),
                        source: media_source(publication.source()),
                    });
                }
            }
        }
        wanted
    }

    fn tell_missing(&self, change: Change) {
        let Change {
            identity,
            source,
            missing,
        } = change;
        if missing {
            tracing::warn!(sfu = %self.sfu, %identity, ?source, "a call participant's media could not be received");
        } else {
            tracing::info!(sfu = %self.sfu, %identity, ?source, "a call participant's media is no longer missing");
        }
        self.emit(MediaEvent::Missing {
            sfu: self.sfu.clone(),
            identity,
            source,
            missing,
        });
    }

    fn quality_changed(&mut self, participant: &Participant, quality: LiveKitQuality) {
        let of = match participant {
            Participant::Local(_) => QualityOf::You,
            Participant::Remote(remote) => QualityOf::Member(identity_of(remote)),
        };
        let quality = match quality {
            LiveKitQuality::Excellent | LiveKitQuality::Good => ConnectionQuality::Good,
            LiveKitQuality::Poor => ConnectionQuality::Poor,
            LiveKitQuality::Lost => ConnectionQuality::Lost,
        };
        if let Some(rated) = self.qualities.rated(of, quality, Instant::now()) {
            self.tell_quality(rated);
        }
    }

    fn hold_ended(&mut self) {
        for rated in self.qualities.due(Instant::now()) {
            self.tell_quality(rated);
        }
    }

    fn tell_quality(&self, (of, quality): Rated) {
        match of {
            QualityOf::You => {
                tracing::info!(sfu = %self.sfu, ?quality, "your call connection changed");
                self.emit(MediaEvent::OwnQuality {
                    sfu: self.sfu.clone(),
                    quality,
                });
            }
            QualityOf::Member(identity) => {
                tracing::info!(sfu = %self.sfu, %identity, ?quality, "a call participant's connection changed");
                self.emit(MediaEvent::Quality {
                    sfu: self.sfu.clone(),
                    identity,
                    quality,
                });
            }
        }
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

const fn media_source(source: TrackSource) -> MediaSource {
    match source {
        TrackSource::Microphone => MediaSource::Microphone,
        TrackSource::Camera => MediaSource::Camera,
        TrackSource::Screenshare => MediaSource::Screen,
        TrackSource::ScreenshareAudio => MediaSource::ScreenAudio,
        TrackSource::Unknown => MediaSource::Unknown,
    }
}

const fn video_kind(source: TrackSource) -> Option<VideoKind> {
    match source {
        TrackSource::Camera => Some(VideoKind::Camera),
        TrackSource::Screenshare => Some(VideoKind::Screen),
        TrackSource::Unknown | TrackSource::Microphone | TrackSource::ScreenshareAudio => None,
    }
}

async fn packets_received(track: &RemoteVideoTrack) -> Option<u64> {
    let stats = track.get_stats().await.ok()?;
    stats.iter().find_map(|stats| match stats {
        RtcStats::InboundRtp(inbound) => Some(inbound.received.packets_received),
        _ => None,
    })
}

fn identity_of(participant: &RemoteParticipant) -> MediaIdentity {
    MediaIdentity::new(participant.identity().to_string())
}
