use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};

use futures_util::StreamExt;
use matrix_sdk::Client;
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::ruma::events::room::message::MessageType;
use matrix_sdk::ruma::{IdParseError, OwnedRoomId};
use matrix_sdk_ui::eyeball_im::VectorDiff;
use matrix_sdk_ui::timeline::{EventTimelineItem, RoomExt, TimelineDetails, TimelineItem};
use tokio::sync::mpsc;

use super::media::{enrich_message, enrich_messages};
use crate::domain::models::{
    EventId, FileMeta, ImageMeta, MessageBody, RoomId, TimelineMessage, TimelinePatch,
};
use crate::error::{AppError, Result};

#[allow(clippy::too_many_lines)]
pub(super) fn convert_event_item(
    event: &EventTimelineItem,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
    own_user_id: Option<&str>,
) -> Option<TimelineMessage> {
    let event_id_str = event
        .event_id()
        .map(ToString::to_string)
        .unwrap_or_default();

    let content = event.content();

    let Some(message) = content.as_message() else {
        if content.as_unable_to_decrypt().is_some() {
            let (sender_display_name, sender_avatar_url) = match event.sender_profile() {
                TimelineDetails::Ready(profile) => (
                    profile.display_name.clone(),
                    profile.avatar_url.as_ref().map(ToString::to_string),
                ),
                _ => (None, None),
            };
            let ts: u64 = event.timestamp().0.into();
            let sender_str = event.sender().to_string();
            let is_own = own_user_id.is_some_and(|uid| uid == sender_str);
            return Some(TimelineMessage {
                event_id: EventId(event_id_str),
                sender: sender_str,
                sender_display_name,
                sender_avatar_url,
                sender_avatar_path: None,
                body: MessageBody::UnableToDecrypt,
                timestamp: ts,
                is_own,
            });
        }
        tracing::debug!(
            event_id = event_id_str,
            sender = %event.sender(),
            "skipping non-message event"
        );
        return None;
    };

    let body = match message.msgtype() {
        MessageType::Text(t) => MessageBody::Text(t.body.clone()),
        MessageType::Notice(n) => MessageBody::Notice(n.body.clone()),
        MessageType::Emote(e) => MessageBody::Emote(e.body.clone()),
        MessageType::Image(i) => {
            if let Ok(mut sources) = media_sources.lock() {
                sources.insert(event_id_str.clone(), i.source.clone());
                if let Some(info) = &i.info
                    && let Some(ref thumb_source) = info.thumbnail_source
                {
                    sources.insert(format!("{event_id_str}:thumb"), thumb_source.clone());
                }
            }
            #[allow(clippy::cast_possible_truncation)]
            let (width, height, mimetype) = i.info.as_ref().map_or((None, None, None), |info| {
                let w = info.width.map(|v| {
                    let n: u64 = v.into();
                    n as u32
                });
                let h = info.height.map(|v| {
                    let n: u64 = v.into();
                    n as u32
                });
                (w, h, info.mimetype.clone())
            });
            MessageBody::Image {
                alt_text: i.body.clone(),
                meta: ImageMeta {
                    width,
                    height,
                    mimetype,
                    thumbnail_path: None,
                },
            }
        }
        MessageType::File(f) => {
            if let Ok(mut sources) = media_sources.lock() {
                sources.insert(event_id_str.clone(), f.source.clone());
            }
            let (mimetype, size) = f.info.as_ref().map_or((None, None), |info| {
                (info.mimetype.clone(), info.size.map(Into::into))
            });
            MessageBody::File {
                meta: FileMeta {
                    filename: f.filename.clone().unwrap_or_else(|| f.body.clone()),
                    mimetype,
                    size,
                },
            }
        }
        other => MessageBody::Unsupported {
            kind: other.msgtype().to_string(),
            fallback: other.body().to_string(),
        },
    };

    let (sender_display_name, sender_avatar_url) = match event.sender_profile() {
        TimelineDetails::Ready(profile) => (
            profile.display_name.clone(),
            profile.avatar_url.as_ref().map(ToString::to_string),
        ),
        _ => (None, None),
    };

    let ts: u64 = event.timestamp().0.into();

    let sender_str = event.sender().to_string();
    let is_own = own_user_id.is_some_and(|uid| uid == sender_str);

    Some(TimelineMessage {
        event_id: EventId(event_id_str),
        sender: sender_str,
        sender_display_name,
        sender_avatar_url,
        sender_avatar_path: None,
        body,
        timestamp: ts,
        is_own,
    })
}

fn convert_timeline_items(
    items: &[Arc<TimelineItem>],
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
    own_user_id: Option<&str>,
) -> Vec<TimelineMessage> {
    items
        .iter()
        .filter_map(|item| convert_event_item(item.as_event()?, media_sources, own_user_id))
        .collect()
}

async fn convert_and_enrich(
    item: &Arc<TimelineItem>,
    client: &Client,
    cache_dir: &Path,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
    own_user_id: Option<&str>,
) -> Option<TimelineMessage> {
    let event = item.as_event()?;
    let mut msg = convert_event_item(event, media_sources, own_user_id)?;
    enrich_message(client, cache_dir, media_sources, &mut msg).await;
    Some(msg)
}

fn msg_index_at(items: &[Arc<TimelineItem>], raw_index: usize) -> usize {
    items
        .get(..raw_index)
        .unwrap_or(items)
        .iter()
        .filter(|i| is_renderable(i))
        .count()
}

fn is_renderable(item: &TimelineItem) -> bool {
    let Some(event) = item.as_event() else {
        return false;
    };
    let content = event.content();
    content.as_message().is_some() || content.as_unable_to_decrypt().is_some()
}

#[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
async fn diff_to_patch(
    items: &mut Vec<Arc<TimelineItem>>,
    diff: VectorDiff<Arc<TimelineItem>>,
    client: &Client,
    cache_dir: &Path,
    media_sources: &StdMutex<HashMap<String, MediaSource>>,
    own_user_id: Option<&str>,
) -> Option<TimelinePatch> {
    match diff {
        VectorDiff::Append { values } => {
            let mut msgs: Vec<TimelineMessage> = values
                .iter()
                .filter_map(|item| {
                    let event = item.as_event()?;
                    convert_event_item(event, media_sources, own_user_id)
                })
                .collect();
            enrich_messages(client, cache_dir, media_sources, &mut msgs).await;
            items.extend(values);
            if msgs.is_empty() {
                return None;
            }
            Some(TimelinePatch::Append(msgs))
        }
        VectorDiff::Clear => {
            items.clear();
            Some(TimelinePatch::Clear)
        }
        VectorDiff::PushFront { value } => {
            let msg =
                convert_and_enrich(&value, client, cache_dir, media_sources, own_user_id).await;
            items.insert(0, value);
            Some(TimelinePatch::PushFront(msg?))
        }
        VectorDiff::PushBack { value } => {
            let msg =
                convert_and_enrich(&value, client, cache_dir, media_sources, own_user_id).await;
            items.push(value);
            Some(TimelinePatch::PushBack(msg?))
        }
        VectorDiff::PopFront => {
            let was_event = items.first().is_some_and(|i| is_renderable(i));
            if !items.is_empty() {
                items.remove(0);
            }
            if was_event {
                Some(TimelinePatch::PopFront)
            } else {
                None
            }
        }
        VectorDiff::PopBack => {
            let was_event = items.last().is_some_and(|i| is_renderable(i));
            items.pop();
            if was_event {
                Some(TimelinePatch::PopBack)
            } else {
                None
            }
        }
        VectorDiff::Insert { index, value } => {
            let msg =
                convert_and_enrich(&value, client, cache_dir, media_sources, own_user_id).await;
            items.insert(index, value);
            let msg = msg?;
            let mi = msg_index_at(items, index);
            Some(TimelinePatch::Insert {
                index: mi,
                message: msg,
            })
        }
        VectorDiff::Set { index, value } => {
            let old_raw = items
                .get(index)
                .and_then(|i| i.as_event())
                .and_then(|e| convert_event_item(e, media_sources, own_user_id));
            let old_mi = if old_raw.is_some() {
                msg_index_at(items, index)
            } else {
                0
            };

            let new_raw = value
                .as_event()
                .and_then(|e| convert_event_item(e, media_sources, own_user_id));

            if let Some(slot) = items.get_mut(index) {
                *slot = Arc::clone(&value);
            }

            match (&old_raw, &new_raw) {
                (Some(old), Some(new)) if old.visually_eq(new) => None,
                (Some(_), Some(_)) => {
                    let new_msg =
                        convert_and_enrich(&value, client, cache_dir, media_sources, own_user_id)
                            .await;
                    Some(TimelinePatch::Set {
                        index: old_mi,
                        message: new_msg?,
                    })
                }
                (Some(_), None) => Some(TimelinePatch::Remove { index: old_mi }),
                (None, Some(_)) => {
                    let mi = msg_index_at(items, index);
                    let new_msg =
                        convert_and_enrich(&value, client, cache_dir, media_sources, own_user_id)
                            .await;
                    Some(TimelinePatch::Insert {
                        index: mi,
                        message: new_msg?,
                    })
                }
                (None, None) => None,
            }
        }
        VectorDiff::Remove { index } => {
            let was_event = items.get(index).is_some_and(|i| is_renderable(i));
            let mi = if was_event {
                msg_index_at(items, index)
            } else {
                0
            };
            items.remove(index);
            if was_event {
                Some(TimelinePatch::Remove { index: mi })
            } else {
                None
            }
        }
        VectorDiff::Truncate { length } => {
            let msg_length = msg_index_at(items, length);
            items.truncate(length);
            Some(TimelinePatch::Truncate { length: msg_length })
        }
        VectorDiff::Reset { values } => {
            *items = values.into_iter().collect();
            let mut msgs = convert_timeline_items(items, media_sources, own_user_id);
            enrich_messages(client, cache_dir, media_sources, &mut msgs).await;
            Some(TimelinePatch::Reset(msgs))
        }
    }
}

pub(super) async fn subscribe_timeline(
    client: &Client,
    cache_dir: &Path,
    media_sources: &Arc<StdMutex<HashMap<String, MediaSource>>>,
    room_id: &RoomId,
    timeline_tx: mpsc::UnboundedSender<TimelinePatch>,
) -> Result<()> {
    let room_id_parsed: OwnedRoomId = room_id
        .0
        .as_str()
        .try_into()
        .map_err(|e: IdParseError| AppError::Other(e.to_string()))?;

    let room = client
        .get_room(&room_id_parsed)
        .ok_or_else(|| AppError::Other("Room not found".into()))?;

    let timeline = Arc::new(
        room.timeline()
            .await
            .map_err(|e| AppError::Other(e.to_string()))?,
    );

    if let Err(e) = timeline.paginate_backwards(50).await {
        tracing::warn!("failed to paginate timeline backwards: {e}");
    }

    let (initial_items, mut stream) = timeline.subscribe().await;

    tokio::spawn({
        let timeline = Arc::clone(&timeline);
        async move { timeline.fetch_members().await }
    });

    let media_sources = Arc::clone(media_sources);
    let cache_dir = cache_dir.join("media-cache");
    let own_user_id = client.user_id().map(ToString::to_string);

    let mut items: Vec<Arc<TimelineItem>> = initial_items.into_iter().collect();
    let mut messages = convert_timeline_items(&items, &media_sources, own_user_id.as_deref());
    enrich_messages(client, &cache_dir, &media_sources, &mut messages).await;
    if timeline_tx.send(TimelinePatch::Reset(messages)).is_err() {
        return Ok(());
    }

    let mut key_stream = std::pin::pin!(
        client
            .encryption()
            .backups()
            .room_keys_for_room_stream(&room_id_parsed)
    );

    let backup_client = client.clone();
    let backup_room_id = room_id_parsed;
    tokio::spawn(async move {
        if let Err(e) = backup_client
            .encryption()
            .backups()
            .download_room_keys_for_room(&backup_room_id)
            .await
        {
            tracing::debug!("backup key download for {backup_room_id}: {e}");
        }
    });

    loop {
        tokio::select! {
            biased;
            diffs = stream.next() => {
                let Some(diffs) = diffs else { break };
                let mut batch = Vec::new();
                for diff in diffs {
                    let patch = diff_to_patch(
                        &mut items,
                        diff,
                        client,
                        &cache_dir,
                        &media_sources,
                        own_user_id.as_deref(),
                    )
                    .await;
                    if let Some(patch) = patch {
                        batch.push(patch);
                    }
                }
                let patch = match batch.len() {
                    0 => continue,
                    1 => batch.remove(0),
                    _ => TimelinePatch::Batch(batch),
                };
                if timeline_tx.send(patch).is_err() {
                    return Ok(());
                }
            }
            result = key_stream.next() => {
                if let Some(Ok(keys)) = result {
                    let session_ids: Vec<String> =
                        keys.into_values().flatten().collect();
                    if !session_ids.is_empty() {
                        timeline.retry_decryption(session_ids).await;
                    }
                }
            }
        }
    }

    Ok(())
}
