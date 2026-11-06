use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures_util::stream::FuturesUnordered;
use matrix_sdk::Room;
use matrix_sdk::deserialized_responses::EncryptionInfo;
use matrix_sdk::ruma::events::{AnySyncTimelineEvent, AnyToDeviceEvent};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{
    MilliSecondsSinceUnixEpoch, OwnedDeviceId, OwnedEventId, OwnedRoomId, OwnedUserId,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::sync::mpsc;

use super::rings::{decline, ring};
use super::{DECLINES_KEPT, Late, SEEN_PER_ROOM};
use crate::adapters::matrix::decrypt_wait::{Attempt, LateDecryption, decrypt_or_wait};
use crate::adapters::matrix::rtc::hands::{
    REACTION_TYPE, REDACTION_TYPE, hand_of, raises_a_hand, redacted_event,
};
use crate::adapters::matrix::rtc::keys::{Arrival, read_key};
use crate::adapters::matrix::rtc::reactions::{CALL_REACTION_TYPE, reaction_of};
use crate::adapters::matrix::rtc::{
    DECLINE_TYPE, DECLINE_TYPE_STABLE, ENCRYPTED_TYPE, KEYS_TYPE, NOTIFICATION_TYPE,
    NOTIFICATION_TYPE_STABLE,
};
use crate::domain::call::ring::{
    CallSignal, MAX_RING_LIFETIME_MS, MAX_SENDER_TS_OFFSET_MS, RingId,
};
use crate::domain::room::RoomId;
use crate::util::unix_now_ms;

pub(super) enum Heard {
    Timeline {
        raw: Raw<AnySyncTimelineEvent>,
        room: Room,
    },
    Signal(CallSignal),
}

#[derive(Deserialize)]
struct Kind {
    #[serde(rename = "type")]
    event_type: String,
    origin_server_ts: MilliSecondsSinceUnixEpoch,
    #[serde(default)]
    content: KindContent,
}

#[derive(Default, Deserialize)]
struct KindContent {
    #[serde(default, rename = "m.relates_to")]
    relates_to: Option<Value>,
}

impl Kind {
    fn concerns_calls(&self) -> bool {
        match self.event_type.as_str() {
            NOTIFICATION_TYPE
            | NOTIFICATION_TYPE_STABLE
            | DECLINE_TYPE
            | DECLINE_TYPE_STABLE
            | REDACTION_TYPE
            | CALL_REACTION_TYPE => true,
            REACTION_TYPE => raises_a_hand(self.content.relates_to.as_ref()),
            ENCRYPTED_TYPE => recent(self.origin_server_ts),
            _ => false,
        }
    }
}

fn recent(sent: MilliSecondsSinceUnixEpoch) -> bool {
    let sent: u64 = sent.0.into();
    sent.saturating_add(MAX_RING_LIFETIME_MS + MAX_SENDER_TS_OFFSET_MS) > unix_now_ms()
}

pub(super) fn pass_on(
    forward: &Mutex<Option<mpsc::UnboundedSender<Heard>>>,
    raw: Raw<AnySyncTimelineEvent>,
    room: Room,
) {
    let Ok(kind) = raw.deserialize_as_unchecked::<Kind>() else {
        return;
    };
    if !kind.concerns_calls() {
        return;
    }
    hand_over(forward, Heard::Timeline { raw, room });
}

fn hand_over(forward: &Mutex<Option<mpsc::UnboundedSender<Heard>>>, heard: Heard) {
    if let Ok(slot) = forward.lock()
        && let Some(heard_tx) = slot.as_ref()
    {
        drop(heard_tx.send(heard));
    }
}

#[derive(Deserialize)]
struct ToDevice {
    sender: OwnedUserId,
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    content: Map<String, Value>,
}

pub(super) fn pass_key(
    forward: &Mutex<Option<mpsc::UnboundedSender<Heard>>>,
    raw: &Raw<AnyToDeviceEvent>,
    info: Option<EncryptionInfo>,
) {
    let Ok(event) = raw.deserialize_as_unchecked::<ToDevice>() else {
        return;
    };
    if event.event_type != KEYS_TYPE {
        return;
    }
    if let Some(device) = olm_sender(&event, info) {
        read_and_pass(forward, &event, &device);
    }
}

fn olm_sender(event: &ToDevice, info: Option<EncryptionInfo>) -> Option<OwnedDeviceId> {
    let sender = &event.sender;
    let room_id = claimed_room(event);
    let Some(info) = info else {
        tracing::info!(%sender, room_id, "dropped a call key that arrived unencrypted");
        return None;
    };
    let device = info.sender_device.filter(|_| info.sender == *sender);
    if device.is_none() {
        tracing::info!(%sender, room_id, "dropped a call key whose sending device is unknown");
    }
    device
}

fn claimed_room(event: &ToDevice) -> &str {
    event
        .content
        .get("room_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn read_and_pass(
    forward: &Mutex<Option<mpsc::UnboundedSender<Heard>>>,
    event: &ToDevice,
    device: &OwnedDeviceId,
) {
    let sender = &event.sender;
    let arrival = Arrival {
        content: &event.content,
        sender: sender.as_str(),
        sender_device: device.as_str(),
        received_ms: unix_now_ms(),
    };
    match read_key(&arrival) {
        Ok((room_id, key)) => {
            tracing::debug!(
                %sender,
                %device,
                room_id,
                index = %key.index,
                bytes = key.key.bytes().len(),
                "a call key arrived"
            );
            hand_over(
                forward,
                Heard::Signal(CallSignal::Key {
                    room_id: RoomId::new(room_id),
                    key: Box::new(key),
                }),
            );
        }
        Err(reason) => {
            let room_id = claimed_room(event);
            tracing::info!(%sender, %device, room_id, reason, "dropped a call key");
        }
    }
}

#[derive(Deserialize)]
pub(super) struct Facts {
    pub(super) event_id: OwnedEventId,
    pub(super) sender: OwnedUserId,
    pub(super) origin_server_ts: MilliSecondsSinceUnixEpoch,
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    redacts: Option<OwnedEventId>,
    #[serde(default)]
    pub(super) content: Map<String, Value>,
    #[serde(default)]
    unsigned: Unsigned,
}

#[derive(Default, Deserialize)]
struct Unsigned {
    age: Option<i64>,
}

#[derive(Clone, Copy)]
struct Received {
    at: Instant,
    age: Option<Duration>,
}

impl Received {
    fn now(facts: &Facts) -> Self {
        Self {
            at: Instant::now(),
            age: facts
                .unsigned
                .age
                .and_then(|age| u64::try_from(age).ok())
                .map(Duration::from_millis),
        }
    }

    fn age(self) -> Option<Duration> {
        self.age.map(|age| age.saturating_add(self.at.elapsed()))
    }
}

pub(super) struct SignalFilter {
    own: OwnedUserId,
    seen: HashMap<OwnedRoomId, VecDeque<OwnedEventId>>,
    declined: VecDeque<(RoomId, RingId)>,
}

impl SignalFilter {
    pub(super) fn new(own: OwnedUserId) -> Self {
        Self {
            own,
            seen: HashMap::new(),
            declined: VecDeque::new(),
        }
    }

    pub(super) fn admit(&mut self, signal: CallSignal) -> Option<CallSignal> {
        match &signal {
            CallSignal::Declined { room_id, ring, by } if by.as_ref() == self.own.as_str() => {
                if self.declined.len() == DECLINES_KEPT {
                    self.declined.pop_front();
                }
                self.declined.push_back((room_id.clone(), ring.clone()));
            }
            CallSignal::Rang(ring)
                if self.declined.iter().any(|(room_id, declined)| {
                    *room_id == ring.room_id && *declined == ring.id
                }) =>
            {
                tracing::debug!(room_id = %ring.room_id, event_id = %ring.id, reason = "you declined it", "a call notification rings nothing");
                return None;
            }
            _ => {}
        }
        Some(signal)
    }

    fn first_sighting(&mut self, room: &Room, event_id: &OwnedEventId) -> bool {
        let seen = self.seen.entry(room.room_id().to_owned()).or_default();
        if seen.contains(event_id) {
            return false;
        }
        if seen.len() == SEEN_PER_ROOM {
            seen.pop_front();
        }
        seen.push_back(event_id.clone());
        true
    }

    pub(super) async fn judge(
        &mut self,
        raw: Raw<AnySyncTimelineEvent>,
        room: Room,
        waiting: &mut FuturesUnordered<Late>,
    ) -> Option<CallSignal> {
        let facts = match raw.deserialize_as_unchecked::<Facts>() {
            Ok(facts) => facts,
            Err(e) => {
                tracing::debug!(room_id = %room.room_id(), "a call signal does not parse: {e}");
                return None;
            }
        };
        if !self.first_sighting(&room, &facts.event_id) {
            return None;
        }
        let received = Received::now(&facts);
        if facts.event_type != ENCRYPTED_TYPE {
            return signal_of(&self.own, &room, facts, received).await;
        }
        match decrypt_or_wait(&room, &raw).await {
            Attempt::Clear(event) => {
                let facts = event.raw().deserialize_as_unchecked::<Facts>().ok()?;
                signal_of(&self.own, &room, facts, received).await
            }
            Attempt::Refused => None,
            Attempt::Later(later) => {
                waiting.push(wait_for_key(self.own.clone(), room, later, received));
                None
            }
        }
    }
}

fn wait_for_key(own: OwnedUserId, room: Room, later: LateDecryption, received: Received) -> Late {
    Box::pin(async move {
        let event = later.await?;
        let facts = event.raw().deserialize_as_unchecked::<Facts>().ok()?;
        signal_of(&own, &room, facts, received).await
    })
}

async fn signal_of(
    own: &OwnedUserId,
    room: &Room,
    facts: Facts,
    received: Received,
) -> Option<CallSignal> {
    match facts.event_type.as_str() {
        NOTIFICATION_TYPE | NOTIFICATION_TYPE_STABLE => ring(own, room, facts).await,
        DECLINE_TYPE | DECLINE_TYPE_STABLE => decline(room, &facts),
        REACTION_TYPE => hand(room, &facts),
        CALL_REACTION_TYPE => call_reaction(room, &facts, received),
        REDACTION_TYPE => redaction(room, &facts),
        _ => None,
    }
}

fn hand(room: &Room, facts: &Facts) -> Option<CallSignal> {
    let sighting = hand_of(
        &facts.event_id,
        &facts.sender,
        facts.origin_server_ts,
        &facts.content,
    )?;
    tracing::debug!(room_id = %room.room_id(), sender = %facts.sender, membership = sighting.membership_event, "a hand was raised");
    Some(CallSignal::Hand {
        room_id: RoomId::new(room.room_id().as_str()),
        sighting,
    })
}

fn call_reaction(room: &Room, facts: &Facts, received: Received) -> Option<CallSignal> {
    let sighting = reaction_of(
        &facts.event_id,
        &facts.sender,
        &facts.content,
        received.age(),
    )?;
    tracing::debug!(room_id = %room.room_id(), sender = %facts.sender, membership = sighting.membership_event, age = ?sighting.age, "a call reaction arrived");
    Some(CallSignal::Reacted {
        room_id: RoomId::new(room.room_id().as_str()),
        sighting,
    })
}

fn redaction(room: &Room, facts: &Facts) -> Option<CallSignal> {
    let event = redacted_event(facts.redacts.as_ref(), &facts.content)?;
    Some(CallSignal::Redacted {
        room_id: RoomId::new(room.room_id().as_str()),
        event,
    })
}
