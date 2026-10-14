use std::collections::{HashMap, HashSet, VecDeque};
use std::future::{self, Future};
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use matrix_sdk::deserialized_responses::{
    RawAnySyncOrStrippedTimelineEvent, TimelineEvent, TimelineEventKind,
};
use matrix_sdk::ruma::events::receipt::{ReceiptThread, ReceiptType};
use matrix_sdk::ruma::events::{AnySyncStateEvent, AnySyncTimelineEvent};
use matrix_sdk::ruma::push::Action;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{
    EventId, MilliSecondsSinceUnixEpoch, OwnedEventId, OwnedRoomId, OwnedUserId,
};
use matrix_sdk::sync::Notification;
use matrix_sdk::{Client, Room, RoomState};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::time::timeout;

use super::build::preview_from_event;
use super::invites::build_invite;
use crate::adapters::matrix::member_name::named_member;
use crate::adapters::matrix::preview::MessagePreview;
use crate::adapters::matrix::session::{ClientHandle, SessionResource};
use crate::domain::message::{MessagePreviewKind, ServiceEvent};
use crate::domain::push::{NoticeContent, PushNotice, PushTweaks};
use crate::domain::room::RoomId;
use crate::error::Result;
use crate::ports::matrix::PushPort;

const CATCH_UP_AGE: Duration = Duration::from_mins(5);
const SEEN_PER_ROOM: usize = 64;
const KEY_WAIT: Duration = Duration::from_secs(3);
const ENCRYPTED_TYPE: &str = "m.room.encrypted";

struct Reported {
    notification: Notification,
    room: Room,
}

type Forward = Arc<Mutex<Option<mpsc::UnboundedSender<Reported>>>>;
type KeyWait = Pin<Box<dyn Future<Output = Option<PushNotice>> + Send>>;

pub(in crate::adapters::matrix) struct MatrixPush {
    matrix: Arc<ClientHandle>,
    forward: Forward,
}

impl MatrixPush {
    pub(in crate::adapters::matrix) async fn new(
        matrix: Arc<ClientHandle>,
        client: &Client,
    ) -> Self {
        let forward = Forward::default();
        let slot = Arc::clone(&forward);
        client
            .register_notification_handler(move |notification, room, _| {
                pass_on(&slot, Reported { notification, room });
                future::ready(())
            })
            .await;
        Self { matrix, forward }
    }

    fn install(&self, reported_tx: mpsc::UnboundedSender<Reported>) {
        if let Ok(mut slot) = self.forward.lock() {
            *slot = Some(reported_tx);
        }
    }
}

fn pass_on(forward: &Mutex<Option<mpsc::UnboundedSender<Reported>>>, reported: Reported) {
    if let Ok(slot) = forward.lock()
        && let Some(reported_tx) = slot.as_ref()
    {
        drop(reported_tx.send(reported));
    }
}

#[async_trait]
impl SessionResource for MatrixPush {
    async fn release(&self) {
        let forward = self.forward.lock().ok().and_then(|mut slot| slot.take());
        drop(forward);
    }
}

#[async_trait]
impl PushPort for MatrixPush {
    async fn follow_notifications(&self, notices_tx: mpsc::Sender<PushNotice>) -> Result<()> {
        let client = self.matrix.client().await?;
        let mut filter = LiveFilter::arm(&client);
        let (reported_tx, mut reported_rx) = mpsc::unbounded_channel();
        self.install(reported_tx);
        tracing::debug!("following notifications");
        let mut waiting: FuturesUnordered<KeyWait> = FuturesUnordered::new();
        loop {
            let notice = tokio::select! {
                reported = reported_rx.recv() => match reported {
                    Some(reported) => judge(&mut filter, reported, &mut waiting).await,
                    None => return Ok(()),
                },
                Some(resolved) = waiting.next(), if !waiting.is_empty() => resolved,
            };
            if let Some(notice) = notice
                && notices_tx.send(notice).await.is_err()
            {
                return Ok(());
            }
        }
    }
}

struct LiveFilter {
    armed_at: u64,
    seen: HashMap<OwnedRoomId, VecDeque<OwnedEventId>>,
    invites: HashSet<OwnedRoomId>,
}

impl LiveFilter {
    fn arm(client: &Client) -> Self {
        Self {
            armed_at: now_ms(),
            seen: HashMap::new(),
            invites: client
                .invited_rooms()
                .iter()
                .map(|room| room.room_id().to_owned())
                .collect(),
        }
    }

    fn first_sighting(&mut self, room: &Room, event_id: &EventId) -> bool {
        let seen = self.seen.entry(room.room_id().to_owned()).or_default();
        if seen.iter().any(|seen_id| seen_id == event_id) {
            return false;
        }
        if seen.len() == SEEN_PER_ROOM {
            seen.pop_front();
        }
        seen.push_back(event_id.to_owned());
        true
    }

    fn first_invite(&mut self, room: &Room) -> bool {
        self.invites.insert(room.room_id().to_owned())
    }

    async fn admit_event(&mut self, room: &Room, raw: &Raw<AnySyncTimelineEvent>) -> Option<Facts> {
        let facts = match raw.deserialize_as_unchecked::<Facts>() {
            Ok(facts) => facts,
            Err(e) => {
                tracing::debug!(room_id = %room.room_id(), "a notification names no event: {e}");
                return None;
            }
        };
        let silence = if facts.sent_at() < self.armed_at {
            Some("it was sent before this session followed notifications")
        } else if facts.arrived_late() {
            Some("it arrived while sync was catching up")
        } else if !self.first_sighting(room, &facts.event_id) {
            Some("it was already seen")
        } else if room.client().is_user_ignored(&facts.sender).await {
            Some("its sender is ignored")
        } else if read_elsewhere(room, &facts.event_id).await {
            Some("it was read on another device")
        } else {
            None
        };
        if let Some(reason) = silence {
            tracing::debug!(room_id = %room.room_id(), event_id = %facts.event_id, reason, "no notification");
            return None;
        }
        Some(facts)
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
    unsigned: Unsigned,
}

#[derive(Default, Deserialize)]
struct Unsigned {
    age: Option<i64>,
}

impl Facts {
    fn sent_at(&self) -> u64 {
        self.origin_server_ts.0.into()
    }

    fn arrived_late(&self) -> bool {
        self.unsigned
            .age
            .and_then(|age| u64::try_from(age).ok())
            .is_some_and(|age| Duration::from_millis(age) > CATCH_UP_AGE)
    }

    fn is_encrypted(&self) -> bool {
        self.event_type == ENCRYPTED_TYPE
    }
}

async fn judge(
    filter: &mut LiveFilter,
    reported: Reported,
    waiting: &mut FuturesUnordered<KeyWait>,
) -> Option<PushNotice> {
    let Reported { notification, room } = reported;
    let tweaks = tweaks_of(&notification.actions);
    match notification.event {
        RawAnySyncOrStrippedTimelineEvent::Sync(raw) => {
            let facts = filter.admit_event(&room, &raw).await?;
            if facts.is_encrypted() {
                return decrypt_or_wait(room, raw, facts, tweaks, waiting).await;
            }
            notice_for_event(&room, &raw, &facts, tweaks).await
        }
        RawAnySyncOrStrippedTimelineEvent::Stripped(_) => {
            if room.state() != RoomState::Invited || !filter.first_invite(&room) {
                return None;
            }
            Some(invite_notice(&room, tweaks).await)
        }
    }
}

fn tweaks_of(actions: &[Action]) -> PushTweaks {
    PushTweaks {
        highlight: actions.iter().any(Action::is_highlight),
        sound: actions.iter().any(|action| action.sound().is_some()),
    }
}

enum Decryption {
    Clear(TimelineEvent),
    KeyMissing(Option<String>),
    Refused,
}

async fn decrypt(room: &Room, raw: &Raw<AnySyncTimelineEvent>) -> Decryption {
    let push_context = room.push_context().await.unwrap_or_else(|e| {
        tracing::debug!(room_id = %room.room_id(), "no push context to judge a decrypted message: {e}");
        None
    });
    match room
        .decrypt_event(raw.cast_ref_unchecked(), push_context.as_ref())
        .await
    {
        Ok(event) => match &event.kind {
            TimelineEventKind::Decrypted(_) => Decryption::Clear(event),
            TimelineEventKind::UnableToDecrypt { utd_info, .. }
                if utd_info.reason.is_missing_room_key() =>
            {
                Decryption::KeyMissing(utd_info.session_id.clone())
            }
            TimelineEventKind::UnableToDecrypt { .. } | TimelineEventKind::PlainText { .. } => {
                Decryption::Refused
            }
        },
        Err(e) => {
            tracing::debug!(room_id = %room.room_id(), "a notified message could not be decrypted: {e}");
            Decryption::Refused
        }
    }
}

async fn decrypt_or_wait(
    room: Room,
    raw: Raw<AnySyncTimelineEvent>,
    facts: Facts,
    tweaks: PushTweaks,
    waiting: &mut FuturesUnordered<KeyWait>,
) -> Option<PushNotice> {
    let keys = room.client().encryption().room_keys_received_stream().await;
    let session_id = match decrypt(&room, &raw).await {
        Decryption::Clear(event) => return clear_notice(&room, &event, &facts, tweaks).await,
        Decryption::Refused => return encrypted_notice(&room, &facts, tweaks).await,
        Decryption::KeyMissing(session_id) => session_id,
    };
    let Some(keys) = keys else {
        return encrypted_notice(&room, &facts, tweaks).await;
    };
    tracing::debug!(room_id = %room.room_id(), event_id = %facts.event_id, "a notified message waits for its room key");
    let room_id = room.room_id().to_owned();
    waiting.push(Box::pin(async move {
        let mut keys = pin!(keys);
        let landed = timeout(KEY_WAIT, async {
            while let Some(received) = keys.next().await {
                let Ok(infos) = received else {
                    return;
                };
                if infos.iter().any(|info| {
                    info.room_id == room_id
                        && session_id.as_ref().is_none_or(|id| *id == info.session_id)
                }) {
                    return;
                }
            }
        })
        .await
        .is_ok();
        tracing::debug!(room_id = %room.room_id(), event_id = %facts.event_id, landed, "a notified message stopped waiting for its room key");
        match decrypt(&room, &raw).await {
            Decryption::Clear(event) => clear_notice(&room, &event, &facts, tweaks).await,
            Decryption::KeyMissing(_) | Decryption::Refused => {
                encrypted_notice(&room, &facts, tweaks).await
            }
        }
    }));
    None
}

async fn clear_notice(
    room: &Room,
    event: &TimelineEvent,
    facts: &Facts,
    tweaks: PushTweaks,
) -> Option<PushNotice> {
    let tweaks = match event.push_actions() {
        Some(actions) if !actions.iter().any(Action::should_notify) => {
            tracing::debug!(room_id = %room.room_id(), event_id = %facts.event_id, "the decrypted message does not notify");
            return None;
        }
        Some(actions) => tweaks_of(actions),
        None => tweaks,
    };
    notice_for_event(room, event.raw(), facts, tweaks).await
}

async fn encrypted_notice(room: &Room, facts: &Facts, tweaks: PushTweaks) -> Option<PushNotice> {
    let preview = MessagePreview::labelled(MessagePreviewKind::Encrypted);
    Some(notice(
        room,
        facts,
        content_of(room, preview, facts).await,
        tweaks,
    ))
}

async fn notice_for_event(
    room: &Room,
    raw: &Raw<AnySyncTimelineEvent>,
    facts: &Facts,
    tweaks: PushTweaks,
) -> Option<PushNotice> {
    let event = match raw.deserialize() {
        Ok(event) => event,
        Err(e) => {
            tracing::debug!(room_id = %room.room_id(), event_id = %facts.event_id, "a notified event does not parse: {e}");
            return None;
        }
    };
    let Some(preview) = notice_preview(&event) else {
        tracing::debug!(room_id = %room.room_id(), event_id = %facts.event_id, "a notified event has no preview");
        return None;
    };
    Some(notice(
        room,
        facts,
        content_of(room, preview, facts).await,
        tweaks,
    ))
}

fn notice_preview(event: &AnySyncTimelineEvent) -> Option<MessagePreview> {
    if let AnySyncTimelineEvent::State(AnySyncStateEvent::RoomTombstone(_)) = event {
        return Some(MessagePreview::service(ServiceEvent::RoomUpgraded));
    }
    preview_from_event(event)
}

async fn content_of(room: &Room, preview: MessagePreview, facts: &Facts) -> NoticeContent {
    if let Some(event) = preview.service {
        return NoticeContent::Service {
            actor: named_member(room, &facts.sender).await,
            event,
        };
    }
    let sender = if room.is_direct().await.unwrap_or_default() {
        None
    } else {
        Some(named_member(room, &facts.sender).await)
    };
    NoticeContent::Message {
        sender,
        kind: preview.kind,
        body: preview.body,
    }
}

fn notice(room: &Room, facts: &Facts, content: NoticeContent, tweaks: PushTweaks) -> PushNotice {
    PushNotice {
        room_id: RoomId::new(room.room_id().as_str()),
        room_name: room_name(room),
        sent_at: facts.sent_at(),
        content,
        tweaks,
    }
}

async fn invite_notice(room: &Room, tweaks: PushTweaks) -> PushNotice {
    let invite = build_invite(room).await;
    PushNotice {
        room_id: invite.id,
        room_name: invite.name,
        sent_at: now_ms(),
        content: NoticeContent::Invite {
            inviter: invite.inviter,
        },
        tweaks,
    }
}

fn room_name(room: &Room) -> String {
    room.cached_display_name()
        .map_or_else(|| room.room_id().to_string(), |name| name.to_string())
}

async fn read_elsewhere(room: &Room, event_id: &EventId) -> bool {
    let own = room.own_user_id();
    for receipt_type in [ReceiptType::Read, ReceiptType::ReadPrivate] {
        for thread in [ReceiptThread::Unthreaded, ReceiptThread::Main] {
            if let Ok(Some((read, _))) = room
                .load_user_receipt(receipt_type.clone(), &thread, own)
                .await
                && *read == *event_id
            {
                return true;
            }
        }
    }
    false
}

fn now_ms() -> u64 {
    MilliSecondsSinceUnixEpoch::now().0.into()
}
