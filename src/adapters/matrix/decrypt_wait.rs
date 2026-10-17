use std::future::Future;
use std::pin::{Pin, pin};
use std::time::Duration;

use futures_util::StreamExt;
use matrix_sdk::Room;
use matrix_sdk::deserialized_responses::{TimelineEvent, TimelineEventKind};
use matrix_sdk::ruma::events::AnySyncTimelineEvent;
use matrix_sdk::ruma::serde::Raw;
use tokio::time::timeout;

const KEY_WAIT: Duration = Duration::from_secs(3);

pub(super) type LateDecryption = Pin<Box<dyn Future<Output = Option<TimelineEvent>> + Send>>;

pub(super) enum Attempt {
    Clear(TimelineEvent),
    Refused,
    Later(LateDecryption),
}

enum Decryption {
    Clear(TimelineEvent),
    KeyMissing(Option<String>),
    Refused,
}

async fn decrypt(room: &Room, raw: &Raw<AnySyncTimelineEvent>) -> Decryption {
    let push_context = room.push_context().await.unwrap_or_else(|e| {
        tracing::debug!(room_id = %room.room_id(), "no push context to judge a decrypted event: {e}");
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
            tracing::debug!(room_id = %room.room_id(), "an event could not be decrypted: {e}");
            Decryption::Refused
        }
    }
}

pub(super) async fn decrypt_or_wait(room: &Room, raw: &Raw<AnySyncTimelineEvent>) -> Attempt {
    let keys = room.client().encryption().room_keys_received_stream().await;
    let session_id = match decrypt(room, raw).await {
        Decryption::Clear(event) => return Attempt::Clear(event),
        Decryption::Refused => return Attempt::Refused,
        Decryption::KeyMissing(session_id) => session_id,
    };
    let Some(keys) = keys else {
        return Attempt::Refused;
    };
    let room = room.clone();
    let raw = raw.clone();
    Attempt::Later(Box::pin(async move {
        let room_id = room.room_id().to_owned();
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
        tracing::debug!(%room_id, landed, "an event stopped waiting for its room key");
        match decrypt(&room, &raw).await {
            Decryption::Clear(event) => Some(event),
            Decryption::KeyMissing(_) | Decryption::Refused => None,
        }
    }))
}
