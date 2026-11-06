mod pump;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use livekit::e2ee::key_provider::KeyProvider;
use livekit::prelude::{RemoteAudioTrack, RemoteTrack, Room, RoomEvent, RoomOptions};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::Instrument;

use self::pump::{Pump, identity_of, pump_events};
use super::e2ee;
use super::feeds::Feeds;
use super::quality::Qualities;
use super::reception::{Flow, Reception};
use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::keys::KeyIndex;
use crate::domain::call::media::MediaEvent;
use crate::domain::call::sfu::{SfuGrant, SfuId, SfuRole, host_of};

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
    let host = host_of(&grant.url).to_owned();
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
