mod received;
mod rings;

use std::future::Future;
use std::mem;
use std::pin::Pin;
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
use matrix_sdk::ruma::events::{AnySyncMessageLikeEvent, AnyToDeviceEvent};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{OwnedDeviceId, OwnedEventId, OwnedUserId};
use matrix_sdk::{Client, Room};
use matrix_sdk_base::crypto::CollectStrategy;
use serde_json::json;
use tokio::sync::mpsc;

use self::received::{Heard, SignalFilter, pass_key, pass_on};
use super::hands::{lower, raise, raised};
use super::keys::{KeyMessage, key_content};
use super::reactions::react;
use super::{KEYS_TYPE, NOTIFICATION_TYPE};
use crate::adapters::matrix::requests::in_time;
use crate::adapters::matrix::session::{ClientHandle, SessionResource};
use crate::domain::call::hands::HandSighting;
use crate::domain::call::reactions::ReactionOption;
use crate::domain::call::ring::{
    CallSignal, OUTGOING_RING_LIFETIME_MS, OutgoingRing, RingId, RingKind,
};
use crate::domain::call::{CallIntent, DeviceStanding, KeySendReport, MemberKey, OutgoingKey};
use crate::domain::room::RoomId;
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

type Forward = Arc<Mutex<Option<mpsc::UnboundedSender<Heard>>>>;

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
        let notification = OwnedEventId::try_from(ring.as_ref()).map_err(AppError::other)?;
        in_time(
            DECLINE_WAIT,
            "the decline was not sent in time",
            room.send(RtcDeclineEventContent::new(notification))
                .with_request_config(RequestConfig::short_retry()),
        )
        .await?;
        tracing::debug!(%room_id, "declined the call");
        Ok(())
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
        let sent = in_time(
            ANNOUNCE_WAIT,
            "the call notification was not sent in time",
            room.send_raw(NOTIFICATION_TYPE, content)
                .with_request_config(RequestConfig::short_retry()),
        )
        .await?;
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
        self.matrix.fetch_avatars(mxcs).await
    }

    async fn raise_hand(&self, room_id: &RoomId, membership_event: &str) -> Result<String> {
        let room = self.matrix.room(room_id).await?;
        raise(&room, membership_event).await
    }

    async fn lower_hand(&self, room_id: &RoomId, reaction: &str) -> Result<()> {
        let room = self.matrix.room(room_id).await?;
        lower(&room, reaction).await
    }

    async fn raised_hands(&self, room_id: &RoomId, memberships: &[String]) -> Vec<HandSighting> {
        match self.matrix.room(room_id).await {
            Ok(room) => raised(&room, memberships).await,
            Err(e) => {
                tracing::debug!(%room_id, "could not ask which hands are raised: {e}");
                Vec::new()
            }
        }
    }

    async fn send_reaction(
        &self,
        room_id: &RoomId,
        membership_event: &str,
        reaction: ReactionOption,
    ) -> Result<String> {
        let room = self.matrix.room(room_id).await?;
        react(&room, membership_event, reaction).await
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
