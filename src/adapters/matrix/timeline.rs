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
                body: MessageBody::Unknown("Unable to decrypt message.".into()),
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
        other => MessageBody::Unknown(other.body().to_string()),
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

async fn diff_to_patch(
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
            if msgs.is_empty() {
                return None;
            }
            Some(TimelinePatch::Append(msgs))
        }
        VectorDiff::Clear => Some(TimelinePatch::Clear),
        VectorDiff::PushFront { value } => {
            let msg =
                convert_and_enrich(&value, client, cache_dir, media_sources, own_user_id).await?;
            Some(TimelinePatch::PushFront(msg))
        }
        VectorDiff::PushBack { value } => {
            let msg =
                convert_and_enrich(&value, client, cache_dir, media_sources, own_user_id).await?;
            Some(TimelinePatch::PushBack(msg))
        }
        VectorDiff::PopFront => Some(TimelinePatch::PopFront),
        VectorDiff::PopBack => Some(TimelinePatch::PopBack),
        VectorDiff::Insert { index, value } => {
            let msg =
                convert_and_enrich(&value, client, cache_dir, media_sources, own_user_id).await?;
            Some(TimelinePatch::Insert {
                index,
                message: msg,
            })
        }
        VectorDiff::Set { index, value } => {
            let msg =
                convert_and_enrich(&value, client, cache_dir, media_sources, own_user_id).await?;
            Some(TimelinePatch::Set {
                index,
                message: msg,
            })
        }
        VectorDiff::Remove { index } => Some(TimelinePatch::Remove { index }),
        VectorDiff::Truncate { length } => Some(TimelinePatch::Truncate { length }),
        VectorDiff::Reset { values } => {
            let mut msgs: Vec<TimelineMessage> = values
                .iter()
                .filter_map(|item| {
                    let event = item.as_event()?;
                    convert_event_item(event, media_sources, own_user_id)
                })
                .collect();
            enrich_messages(client, cache_dir, media_sources, &mut msgs).await;
            Some(TimelinePatch::Reset(msgs))
        }
    }
}

pub(super) async fn subscribe_timeline(
    client: &Client,
    data_dir: &Path,
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

    let timeline = room
        .timeline()
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;

    if let Err(e) = timeline.paginate_backwards(50).await {
        tracing::warn!("failed to paginate timeline backwards: {e}");
    }

    let (initial_items, mut stream) = timeline.subscribe().await;

    let media_sources = Arc::clone(media_sources);
    let cache_dir = data_dir.join("media-cache");
    let own_user_id = client.user_id().map(ToString::to_string);

    let initial_vec: Vec<Arc<TimelineItem>> = initial_items.into_iter().collect();
    let mut messages = convert_timeline_items(&initial_vec, &media_sources, own_user_id.as_deref());
    enrich_messages(client, &cache_dir, &media_sources, &mut messages).await;
    if timeline_tx.send(TimelinePatch::Reset(messages)).is_err() {
        return Ok(());
    }

    let backup_client = client.clone();
    let backup_room_id = room_id_parsed.clone();
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

    while let Some(diffs) = stream.next().await {
        for diff in diffs {
            let patch = diff_to_patch(
                diff,
                client,
                &cache_dir,
                &media_sources,
                own_user_id.as_deref(),
            )
            .await;
            if let Some(patch) = patch
                && timeline_tx.send(patch).is_err()
            {
                return Ok(());
            }
        }
    }

    Ok(())
}
