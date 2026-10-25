use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::mem;
use std::pin::Pin;
use std::result;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use matrix_sdk::config::RequestConfig;
use matrix_sdk::deserialized_responses::EncryptionInfo;
use matrix_sdk::encryption::identities::Device;
use matrix_sdk::event_handler::EventHandlerDropGuard;
use matrix_sdk::ruma::events::rtc::decline::RtcDeclineEventContent;
use matrix_sdk::ruma::events::{AnySyncMessageLikeEvent, AnySyncTimelineEvent, AnyToDeviceEvent};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{
    MilliSecondsSinceUnixEpoch, OwnedDeviceId, OwnedEventId, OwnedRoomId, OwnedUserId,
};
use matrix_sdk::{Client, Room};
use matrix_sdk_base::crypto::CollectStrategy;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;
use tokio::time::timeout;

use super::keys::{Arrival, KeyMessage, key_content, read_key};
use super::members::{Referenced, referenced_membership};
use super::{
    DECLINE_TYPE, DECLINE_TYPE_STABLE, ENCRYPTED_TYPE, KEYS_TYPE, NOTIFICATION_TYPE,
    NOTIFICATION_TYPE_STABLE,
};
use crate::adapters::matrix::decrypt_wait::{Attempt, LateDecryption, decrypt_or_wait};
use crate::adapters::matrix::media::fetch_avatar_thumbnails;
use crate::adapters::matrix::member_name::named_member;
use crate::adapters::matrix::session::{ClientHandle, SessionResource};
use crate::domain::call::ring::{
    CallSignal, MAX_RING_LIFETIME_MS, MAX_SENDER_TS_OFFSET_MS, OUTGOING_RING_LIFETIME_MS,
    OutgoingRing, Ring, RingId, RingKind, expires_ms,
};
use crate::domain::call::{
    CallIntent, DeviceStanding, KeySendReport, MemberKey, MemberSession, OutgoingKey,
};
use crate::domain::room::RoomId;
use crate::domain::user_info::UserId;
use crate::error::{AppError, Result};
use crate::ports::matrix::CallSignalPort;

const SEEN_PER_ROOM: usize = 32;
const DECLINES_KEPT: usize = 32;
const DECLINE_WAIT: Duration = Duration::from_secs(5);
const ANNOUNCE_WAIT: Duration = Duration::from_secs(10);
const RING_TYPE: &str = "ring";
const NOTIFY_TYPE: &str = "notification";
const AUDIO_INTENT: &str = "audio";
const VIDEO_INTENT: &str = "video";
const ROOM_SLOT: &str = "m.call#ROOM";
const STICKY_KEY: &str = "msc4354_sticky_key";
const REFERENCE: &str = "m.reference";

enum Heard {
    Timeline {
        raw: Raw<AnySyncTimelineEvent>,
        room: Room,
    },
    Signal(CallSignal),
}

type Forward = Arc<Mutex<Option<mpsc::UnboundedSender<Heard>>>>;

#[derive(Deserialize)]
struct Kind {
    #[serde(rename = "type")]
    event_type: String,
    origin_server_ts: MilliSecondsSinceUnixEpoch,
}

impl Kind {
    fn concerns_calls(&self) -> bool {
        match self.event_type.as_str() {
            NOTIFICATION_TYPE | NOTIFICATION_TYPE_STABLE | DECLINE_TYPE | DECLINE_TYPE_STABLE => {
                true
            }
            ENCRYPTED_TYPE => recent(self.origin_server_ts),
            _ => false,
        }
    }
}

fn now_ms() -> u64 {
    MilliSecondsSinceUnixEpoch::now().0.into()
}

fn recent(sent: MilliSecondsSinceUnixEpoch) -> bool {
    let sent: u64 = sent.0.into();
    sent.saturating_add(MAX_RING_LIFETIME_MS + MAX_SENDER_TS_OFFSET_MS) > now_ms()
}

pub(in crate::adapters::matrix) struct MatrixCallSignals {
    matrix: Arc<ClientHandle>,
    forward: Forward,
    listeners: Mutex<Vec<EventHandlerDropGuard>>,
}

impl MatrixCallSignals {
    pub(in crate::adapters::matrix) fn new(matrix: Arc<ClientHandle>, client: &Client) -> Self {
        let forward = Forward::default();
        let timeline = client.add_event_handler({
            let forward = Arc::clone(&forward);
            move |raw: Raw<AnySyncMessageLikeEvent>, room: Room| {
                let forward = Arc::clone(&forward);
                async move { pass_on(&forward, raw.cast_unchecked(), room) }
            }
        });
        let to_device = client.add_event_handler({
            let forward = Arc::clone(&forward);
            move |raw: Raw<AnyToDeviceEvent>, info: Option<EncryptionInfo>| {
                let forward = Arc::clone(&forward);
                async move { pass_key(&forward, &raw, info) }
            }
        });
        Self {
            matrix,
            forward,
            listeners: Mutex::new(vec![
                client.event_handler_drop_guard(timeline),
                client.event_handler_drop_guard(to_device),
            ]),
        }
    }

    fn install(&self, heard_tx: mpsc::UnboundedSender<Heard>) {
        if let Ok(mut slot) = self.forward.lock() {
            *slot = Some(heard_tx);
        }
    }
}

fn pass_on(
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

fn pass_key(
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
        received_ms: now_ms(),
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

#[async_trait]
impl SessionResource for MatrixCallSignals {
    async fn release(&self) {
        let forward = self.forward.lock().ok().and_then(|mut slot| slot.take());
        drop(forward);
        let listeners = self
            .listeners
            .lock()
            .map(|mut listeners| mem::take(&mut *listeners))
            .unwrap_or_default();
        drop(listeners);
    }
}

type Late = Pin<Box<dyn Future<Output = Option<CallSignal>> + Send>>;

#[async_trait]
impl CallSignalPort for MatrixCallSignals {
    async fn follow_signals(&self, signals_tx: mpsc::Sender<CallSignal>) -> Result<()> {
        let client = self.matrix.client().await?;
        let own = client
            .user_id()
            .map(ToOwned::to_owned)
            .ok_or_else(|| AppError::Other("the session has no user".into()))?;
        let mut filter = SignalFilter::new(own);
        let (heard_tx, mut heard_rx) = mpsc::unbounded_channel();
        self.install(heard_tx);
        tracing::debug!("following call signals");
        let mut waiting: FuturesUnordered<Late> = FuturesUnordered::new();
        loop {
            let signal = tokio::select! {
                heard = heard_rx.recv() => match heard {
                    Some(Heard::Timeline { raw, room }) => filter.judge(raw, room, &mut waiting).await,
                    Some(Heard::Signal(signal)) => Some(signal),
                    None => return Ok(()),
                },
                Some(resolved) = waiting.next(), if !waiting.is_empty() => resolved,
            };
            if let Some(signal) = signal.and_then(|signal| filter.admit(signal))
                && signals_tx.send(signal).await.is_err()
            {
                return Ok(());
            }
        }
    }

    async fn decline(&self, room_id: &RoomId, ring: &RingId) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        let notification =
            OwnedEventId::try_from(ring.as_ref()).map_err(|e| AppError::Other(e.to_string()))?;
        let sent = timeout(
            DECLINE_WAIT,
            room.send(RtcDeclineEventContent::new(notification))
                .with_request_config(RequestConfig::short_retry()),
        )
        .await;
        match sent {
            Ok(Ok(_)) => {
                tracing::debug!(%room_id, "declined the call");
                Ok(())
            }
            Ok(Err(e)) => Err(AppError::Other(e.to_string())),
            Err(_) => Err(AppError::Other("the decline was not sent in time".into())),
        }
    }

    async fn announce_call(&self, room_id: &RoomId, ring: &OutgoingRing) -> Result<RingId> {
        let room = self.matrix.room(room_id).await?;
        let content = json!({
            "slot_id": ROOM_SLOT,
            "m.mentions": { "user_ids": [], "room": true },
            "notification_type": match ring.kind {
                RingKind::Ring => RING_TYPE,
                RingKind::Notify => NOTIFY_TYPE,
            },
            "m.relates_to": { "rel_type": "m.reference", "event_id": ring.membership_event_id },
            "sender_ts": ring.sent_ms,
            "lifetime": OUTGOING_RING_LIFETIME_MS,
            "m.call.intent": intent_word(ring.intent),
        });
        let sent = timeout(
            ANNOUNCE_WAIT,
            room.send_raw(NOTIFICATION_TYPE, content)
                .with_request_config(RequestConfig::short_retry()),
        )
        .await
        .map_err(|_| AppError::Other("the call notification was not sent in time".into()))?
        .map_err(|e| AppError::Other(e.to_string()))?;
        Ok(RingId::new(sent.response.event_id.as_str()))
    }

    async fn send_key(
        &self,
        room_id: &RoomId,
        key: &OutgoingKey,
        to: &[MemberKey],
    ) -> KeySendReport {
        let everyone_withheld = || KeySendReport {
            sent: Vec::new(),
            withheld: to.to_vec(),
        };
        let Ok(client) = self.matrix.client().await else {
            return everyone_withheld();
        };
        let Some(device) = client.device_id() else {
            return everyone_withheld();
        };
        let content = key_content(&KeyMessage {
            room_id: room_id.as_ref(),
            device: device.as_str(),
            member_id: &key.member_id,
            index: key.index,
            key: &key.key,
            sent_ms: key.sent_ms,
        });
        let Ok(raw) = Raw::new(&content).map(Raw::cast_unchecked) else {
            return everyone_withheld();
        };
        let (known, mut withheld) = devices_of(&client, to).await;
        let recipients: Vec<&Device> = known.iter().map(|(_, device)| device).collect();
        let failed = match client
            .encryption()
            .encrypt_and_send_raw_to_device(
                recipients,
                KEYS_TYPE,
                raw,
                CollectStrategy::IdentityBasedStrategy,
            )
            .await
        {
            Ok(failed) => failed,
            Err(e) => {
                tracing::warn!(%room_id, "the call key was not sent: {e}");
                return everyone_withheld();
            }
        };
        let mut sent = Vec::new();
        for (member, device) in known {
            let missed = failed
                .iter()
                .any(|(user, id)| *user == device.user_id() && *id == device.device_id());
            if missed {
                withheld.push(member);
            } else {
                sent.push(member);
            }
        }
        KeySendReport { sent, withheld }
    }

    async fn device_standing(&self, members: &[MemberKey]) -> Vec<(MemberKey, DeviceStanding)> {
        let Ok(client) = self.matrix.client().await else {
            return members
                .iter()
                .map(|member| (member.clone(), DeviceStanding::Unknown))
                .collect();
        };
        let (known, unknown) = devices_of(&client, members).await;
        let mut standings: Vec<(MemberKey, DeviceStanding)> = known
            .into_iter()
            .map(|(member, device)| {
                let standing = if device.is_cross_signed_by_owner() {
                    DeviceStanding::CrossSigned
                } else {
                    DeviceStanding::NotCrossSigned
                };
                (member, standing)
            })
            .collect();
        standings.extend(
            unknown
                .into_iter()
                .map(|member| (member, DeviceStanding::Unknown)),
        );
        standings
    }

    async fn fetch_avatars(&self, mxcs: &[String]) -> usize {
        let Ok(client) = self.matrix.client().await else {
            return 0;
        };
        fetch_avatar_thumbnails(&client, self.matrix.media(), mxcs).await
    }
}

async fn devices_of(
    client: &Client,
    members: &[MemberKey],
) -> (Vec<(MemberKey, Device)>, Vec<MemberKey>) {
    let mut known = Vec::new();
    let mut unknown = Vec::new();
    for member in members {
        let Ok(user) = OwnedUserId::try_from(member.user.as_str()) else {
            unknown.push(member.clone());
            continue;
        };
        let device_id = OwnedDeviceId::from(member.device.as_ref());
        match client.encryption().get_device(&user, &device_id).await {
            Ok(Some(device)) => known.push((member.clone(), device)),
            Ok(None) => unknown.push(member.clone()),
            Err(e) => {
                tracing::debug!(%member, "could not read a call member's device: {e}");
                unknown.push(member.clone());
            }
        }
    }
    (known, unknown)
}

const fn intent_word(intent: CallIntent) -> &'static str {
    match intent {
        CallIntent::Voice => AUDIO_INTENT,
        CallIntent::Video => VIDEO_INTENT,
    }
}

#[derive(Deserialize)]
struct Facts {
    event_id: OwnedEventId,
    sender: OwnedUserId,
    origin_server_ts: MilliSecondsSinceUnixEpoch,
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    content: Map<String, Value>,
}

struct SignalFilter {
    own: OwnedUserId,
    seen: HashMap<OwnedRoomId, VecDeque<OwnedEventId>>,
    declined: VecDeque<(RoomId, RingId)>,
}

impl SignalFilter {
    fn new(own: OwnedUserId) -> Self {
        Self {
            own,
            seen: HashMap::new(),
            declined: VecDeque::new(),
        }
    }

    fn admit(&mut self, signal: CallSignal) -> Option<CallSignal> {
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

    async fn judge(
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
        if facts.event_type != ENCRYPTED_TYPE {
            return signal_of(&self.own, &room, facts).await;
        }
        match decrypt_or_wait(&room, &raw).await {
            Attempt::Clear(event) => {
                let facts = event.raw().deserialize_as_unchecked::<Facts>().ok()?;
                signal_of(&self.own, &room, facts).await
            }
            Attempt::Refused => None,
            Attempt::Later(later) => {
                waiting.push(wait_for_key(self.own.clone(), room, later));
                None
            }
        }
    }
}

fn wait_for_key(own: OwnedUserId, room: Room, later: LateDecryption) -> Late {
    Box::pin(async move {
        let event = later.await?;
        let facts = event.raw().deserialize_as_unchecked::<Facts>().ok()?;
        signal_of(&own, &room, facts).await
    })
}

async fn signal_of(own: &OwnedUserId, room: &Room, facts: Facts) -> Option<CallSignal> {
    match facts.event_type.as_str() {
        NOTIFICATION_TYPE | NOTIFICATION_TYPE_STABLE => ring(own, room, facts).await,
        DECLINE_TYPE | DECLINE_TYPE_STABLE => decline(room, &facts),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Named,
    Unnamed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Audience {
    You,
    Room,
}

struct Invitation {
    kind: RingKind,
    intent: Option<CallIntent>,
    expires_ms: u64,
    slot: Slot,
    audience: Audience,
    membership: Option<OwnedEventId>,
}

async fn ring(own: &OwnedUserId, room: &Room, facts: Facts) -> Option<CallSignal> {
    let room_id = room.room_id();
    let event_id = &facts.event_id;
    let checked = match invitation(own, &facts, now_ms()) {
        Ok(invitation) => confirm(room, &facts, &invitation)
            .await
            .map(|session| (invitation, session)),
        Err(reason) => Err(reason),
    };
    let (invitation, session) = match checked {
        Ok(checked) => checked,
        Err(reason) => {
            tracing::debug!(%room_id, %event_id, reason, "a call notification rings nothing");
            return None;
        }
    };
    tracing::debug!(%room_id, %event_id, kind = ?invitation.kind, "a call notification arrived");
    Some(CallSignal::Rang(Box::new(Ring {
        id: RingId::new(event_id.as_str()),
        room_id: RoomId::new(room_id.as_str()),
        caller: named_member(room, &facts.sender).await,
        session,
        intent: invitation.intent,
        kind: invitation.kind,
        expires_ms: invitation.expires_ms,
    })))
}

fn invitation(
    own: &OwnedUserId,
    facts: &Facts,
    now_ms: u64,
) -> result::Result<Invitation, &'static str> {
    if facts.sender == *own {
        return Err("you sent it");
    }
    let content = &facts.content;
    if content.keys().all(|key| key == STICKY_KEY) {
        return Err("it was withdrawn");
    }
    let slot = slot_of(content)?;
    if content
        .get(STICKY_KEY)
        .is_some_and(|key| key.as_str() != Some(ROOM_SLOT))
    {
        return Err("its sticky key is not its slot");
    }
    let sender_ts = content
        .get("sender_ts")
        .and_then(Value::as_u64)
        .ok_or("its sender_ts is not a whole number")?;
    let lifetime = content
        .get("lifetime")
        .and_then(Value::as_u64)
        .ok_or("its lifetime is not a whole number")?;
    let expires = expires_ms(sender_ts, facts.origin_server_ts.0.into(), lifetime);
    if expires <= now_ms {
        return Err("it has expired");
    }
    let audience = audience_of(own, content)?;
    let membership = membership_of(content)?;
    if slot == Slot::Unnamed && membership.is_none() {
        return Err("it names no slot and references no membership");
    }
    Ok(Invitation {
        kind: kind_of(content),
        intent: intent(content),
        expires_ms: expires,
        slot,
        audience,
        membership,
    })
}

fn slot_of(content: &Map<String, Value>) -> result::Result<Slot, &'static str> {
    match content.get("slot_id") {
        None => Ok(Slot::Unnamed),
        Some(Value::String(slot)) if slot == ROOM_SLOT => Ok(Slot::Named),
        Some(Value::String(slot)) if slot.split('#').count() == 2 => {
            Err("it invites to another slot")
        }
        Some(_) => Err("its slot_id is malformed"),
    }
}

fn audience_of(
    own: &OwnedUserId,
    content: &Map<String, Value>,
) -> result::Result<Audience, &'static str> {
    const MALFORMED: &str = "its m.mentions is malformed";
    let Some(mentions) = content.get("m.mentions") else {
        return Ok(Audience::Room);
    };
    let mentions = mentions.as_object().ok_or(MALFORMED)?;
    let room = match mentions.get("room") {
        None => false,
        Some(room) => room.as_bool().ok_or(MALFORMED)?,
    };
    let users: Vec<&str> = match mentions.get("user_ids") {
        None => Vec::new(),
        Some(users) => users
            .as_array()
            .and_then(|users| users.iter().map(Value::as_str).collect())
            .ok_or(MALFORMED)?,
    };
    if users.contains(&own.as_str()) {
        Ok(Audience::You)
    } else if room {
        Ok(Audience::Room)
    } else if users.is_empty() {
        Err("it mentions nobody")
    } else {
        Err("it mentions someone else")
    }
}

fn membership_of(
    content: &Map<String, Value>,
) -> result::Result<Option<OwnedEventId>, &'static str> {
    let Some(relation) = content.get("m.relates_to") else {
        return Ok(None);
    };
    relation
        .as_object()
        .filter(|relation| relation.get("rel_type").and_then(Value::as_str) == Some(REFERENCE))
        .and_then(|relation| relation.get("event_id"))
        .and_then(Value::as_str)
        .and_then(|event_id| OwnedEventId::try_from(event_id).ok())
        .map(Some)
        .ok_or("its m.relates_to is no reference to an event")
}

fn kind_of(content: &Map<String, Value>) -> RingKind {
    if content.get("notification_type").and_then(Value::as_str) == Some(RING_TYPE) {
        RingKind::Ring
    } else {
        RingKind::Notify
    }
}

async fn confirm(
    room: &Room,
    facts: &Facts,
    invitation: &Invitation,
) -> result::Result<Option<MemberSession>, &'static str> {
    if invitation.audience == Audience::Room
        && !room
            .power_levels_or_default()
            .await
            .user_can_trigger_room_notification(&facts.sender)
    {
        return Err("its sender may not notify the whole room");
    }
    let Some(membership) = &invitation.membership else {
        return Ok(None);
    };
    let referenced = match referenced_membership(room, membership, &facts.sender, now_ms()).await {
        Ok(referenced) => referenced,
        Err(reason) => {
            tracing::debug!(%membership, reason, "the membership a call notification references does not count");
            return Err("its membership does not count");
        }
    };
    match (referenced, invitation.slot) {
        (Referenced::Current(session), _) | (Referenced::Replaced(session), Slot::Named) => {
            Ok(Some(session))
        }
        (Referenced::Replaced(_), Slot::Unnamed) => {
            Err("it names no slot and its membership is no longer current")
        }
        (Referenced::Unknown, Slot::Named) => Ok(None),
        (Referenced::Unknown, Slot::Unnamed) => {
            Err("it names no slot and its membership could not be read")
        }
    }
}

fn decline(room: &Room, facts: &Facts) -> Option<CallSignal> {
    let ring = facts
        .content
        .get("m.relates_to")
        .and_then(Value::as_object)
        .and_then(|relation| relation.get("event_id"))
        .and_then(Value::as_str)?;
    tracing::debug!(room_id = %room.room_id(), sender = %facts.sender, "a call was declined");
    Some(CallSignal::Declined {
        room_id: RoomId::new(room.room_id().as_str()),
        ring: RingId::new(ring),
        by: UserId::new(facts.sender.as_str()),
    })
}

fn intent(content: &Map<String, Value>) -> Option<CallIntent> {
    match content.get("m.call.intent").and_then(Value::as_str)? {
        AUDIO_INTENT => Some(CallIntent::Voice),
        VIDEO_INTENT => Some(CallIntent::Video),
        _ => None,
    }
}
