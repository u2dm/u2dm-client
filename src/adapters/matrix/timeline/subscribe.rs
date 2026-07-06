use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use futures_util::StreamExt;
use matrix_sdk::Client;
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::ruma::{IdParseError, OwnedEventId, OwnedRoomId};
use matrix_sdk_ui::eyeball_im::VectorDiff;
use matrix_sdk_ui::timeline::{RoomExt as _, Timeline, TimelineItem};
use tokio::fs;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use super::TimelineContext;
use super::diff::diff_to_patch;
use super::filter::convert_timeline_items;
use crate::adapters::matrix::media::{enrich_message, needs_media_download, try_enrich_from_cache};
use crate::domain::models::{
    PaginationDirection, RoomId, TimelineCommand, TimelineMessage, TimelinePatch, TimelineUpdate,
};
use crate::domain::viewport::PAGINATION_BATCH_SIZE;
use crate::error::{AppError, Result};

fn spawn_media_enrichment(
    client: &Client,
    media_dir: &Path,
    media_sources: &Arc<StdMutex<HashMap<String, MediaSource>>>,
    materialized: &Arc<StdMutex<HashMap<String, PathBuf>>>,
    timeline_tx: &mpsc::UnboundedSender<TimelineUpdate>,
    msg: &TimelineMessage,
) {
    let mut msg = msg.clone();
    let client = client.clone();
    let media_dir = media_dir.to_path_buf();
    let media_sources = Arc::clone(media_sources);
    let materialized = Arc::clone(materialized);
    let tx = timeline_tx.clone();
    tokio::spawn(async move {
        enrich_message(&client, &media_dir, &media_sources, &materialized, &mut msg).await;
        drop(tx.send(TimelineUpdate::Patch(TimelinePatch::UpdateMedia {
            event_id: msg.event_id.clone(),
            message: msg,
        })));
    });
}

pub(super) fn spawn_enrichment_for_messages(
    messages: &[TimelineMessage],
    ctx: &TimelineContext<'_>,
) {
    for msg in messages {
        if needs_media_download(msg) {
            spawn_media_enrichment(
                ctx.client,
                ctx.media_dir,
                ctx.media_sources,
                ctx.materialized,
                ctx.timeline_tx,
                msg,
            );
        }
    }
}

fn send_initial_timeline(
    items: &[Arc<TimelineItem>],
    ctx: &TimelineContext<'_>,
    room_id: &RoomId,
    timeline_tx: &mpsc::UnboundedSender<TimelineUpdate>,
) -> bool {
    let mut messages = convert_timeline_items(items, ctx);
    tracing::info!(
        raw_items = items.len(),
        messages = messages.len(),
        %room_id,
        "timeline loaded"
    );
    try_enrich_from_cache(ctx.materialized, &mut messages);
    tracing::debug!(
        messages = messages.len(),
        %room_id,
        "sending initial Reset patch to timeline channel"
    );
    let sent = timeline_tx
        .send(TimelineUpdate::Patch(TimelinePatch::Reset(
            messages.clone(),
        )))
        .is_ok();
    tracing::debug!(sent, %room_id, "initial Reset patch send result");
    if sent {
        spawn_enrichment_for_messages(&messages, ctx);
    }
    sent
}

fn process_diffs(
    items: &mut Vec<Arc<TimelineItem>>,
    diffs: Vec<VectorDiff<Arc<TimelineItem>>>,
    ctx: &TimelineContext<'_>,
) -> Option<TimelinePatch> {
    tracing::debug!(num_diffs = diffs.len(), "processing incoming diffs");
    let mut batch = Vec::new();
    for diff in diffs {
        if let Some(patch) = diff_to_patch(items, diff, ctx) {
            tracing::debug!(patch = patch.label(), "diff produced patch");
            batch.push(patch);
        }
    }
    let result = match batch.len() {
        0 => None,
        1 => Some(batch.remove(0)),
        _ => Some(TimelinePatch::Batch(batch)),
    };
    tracing::debug!(
        produced = result.is_some(),
        label = result.as_ref().map(TimelinePatch::label),
        "process_diffs result"
    );
    result
}

fn spawn_backup_key_download(
    side_tasks: &mut JoinSet<()>,
    client: &Client,
    room_id_parsed: &OwnedRoomId,
) {
    let backup_client = client.clone();
    let backup_room_id = room_id_parsed.clone();
    side_tasks.spawn(async move {
        if let Err(e) = backup_client
            .encryption()
            .backups()
            .download_room_keys_for_room(&backup_room_id)
            .await
        {
            tracing::debug!("backup key download for {backup_room_id}: {e}");
        }
    });
}

async fn ensure_media_dirs(media_dir: &Path) {
    if let Err(e) = fs::create_dir_all(media_dir).await {
        tracing::warn!("failed to create media dir: {e}");
    }
    let avatar_dir = media_dir.join("avatars");
    if let Err(e) = fs::create_dir_all(&avatar_dir).await {
        tracing::warn!("failed to create avatar dir: {e}");
    }
}

async fn handle_timeline_command(
    cmd: TimelineCommand,
    timeline: &Timeline,
    timeline_tx: &mpsc::UnboundedSender<TimelineUpdate>,
) {
    let (direction, hit_end) = match cmd {
        TimelineCommand::PaginateBackwards => (
            PaginationDirection::Backwards,
            paginate_backwards(timeline).await,
        ),
        TimelineCommand::PaginateForwards => (
            PaginationDirection::Forwards,
            paginate_forwards(timeline).await,
        ),
    };

    if timeline_tx
        .send(TimelineUpdate::Pagination { direction, hit_end })
        .is_err()
    {
        tracing::debug!("timeline update channel closed");
    }
}

async fn paginate_backwards(timeline: &Timeline) -> bool {
    tracing::debug!("paginating backwards");
    match timeline.paginate_backwards(PAGINATION_BATCH_SIZE).await {
        Ok(hit_start) => hit_start,
        Err(e) => {
            tracing::warn!("backward pagination failed: {e}");
            false
        }
    }
}

async fn paginate_forwards(timeline: &Timeline) -> bool {
    tracing::debug!("paginating forwards");
    match timeline.paginate_forwards(PAGINATION_BATCH_SIZE).await {
        Ok(hit_end) => hit_end,
        Err(e) => {
            tracing::warn!("forward pagination failed: {e}");
            false
        }
    }
}

async fn setup_timeline(
    client: &Client,
    room_id: &RoomId,
) -> Result<(Arc<Timeline>, OwnedRoomId, bool)> {
    let room_id_parsed: OwnedRoomId = room_id
        .as_ref()
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

    let backwards_ended = paginate_backwards(&timeline).await;

    Ok((timeline, room_id_parsed, backwards_ended))
}

fn spawn_reply_detail_fetches(
    items: &[Arc<TimelineItem>],
    timeline: &Arc<Timeline>,
    fetched: &mut HashSet<String>,
    side_tasks: &mut JoinSet<()>,
) {
    for item in items {
        let Some(event) = item.as_event() else {
            continue;
        };
        let Some(details) = event.content().in_reply_to() else {
            continue;
        };
        if !details.event.is_unavailable() {
            continue;
        }
        let Some(event_id) = event.event_id().map(ToString::to_string) else {
            continue;
        };
        if !fetched.insert(event_id.clone()) {
            continue;
        }
        let timeline = Arc::clone(timeline);
        side_tasks.spawn(async move {
            let Ok(id) = OwnedEventId::try_from(event_id.as_str()) else {
                return;
            };
            if let Err(e) = timeline.fetch_details_for_event(&id).await {
                tracing::debug!("failed to fetch reply details: {e}");
            }
        });
    }
}

async fn handle_room_keys(timeline: &Timeline, keys: BTreeMap<String, BTreeSet<String>>) {
    let session_ids: Vec<String> = keys.into_values().flatten().collect();
    if !session_ids.is_empty() {
        timeline.retry_decryption(session_ids).await;
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn subscribe_timeline(
    client: &Client,
    media_dir: &Path,
    media_sources: &Arc<StdMutex<HashMap<String, MediaSource>>>,
    materialized: &Arc<StdMutex<HashMap<String, PathBuf>>>,
    room_id: &RoomId,
    timeline_tx: mpsc::UnboundedSender<TimelineUpdate>,
    mut cmd_rx: mpsc::UnboundedReceiver<TimelineCommand>,
) -> Result<()> {
    let (timeline, room_id_parsed, backwards_ended) = setup_timeline(client, room_id).await?;

    ensure_media_dirs(media_dir).await;

    let (initial_items, mut stream) = timeline.subscribe().await;

    let mut side_tasks = JoinSet::new();
    side_tasks.spawn({
        let timeline = Arc::clone(&timeline);
        async move { timeline.fetch_members().await }
    });

    let own_user_id = client.user_id().map(ToString::to_string);
    let ctx = TimelineContext {
        client,
        media_dir,
        media_sources,
        materialized,
        own_user_id: own_user_id.as_deref(),
        timeline_tx: &timeline_tx,
    };

    let mut items: Vec<Arc<TimelineItem>> = initial_items.into_iter().collect();
    if !send_initial_timeline(&items, &ctx, room_id, &timeline_tx) {
        return Ok(());
    }
    if timeline_tx
        .send(TimelineUpdate::Pagination {
            direction: PaginationDirection::Backwards,
            hit_end: backwards_ended,
        })
        .is_err()
    {
        return Ok(());
    }

    let mut fetched_reply_details: HashSet<String> = HashSet::new();
    spawn_reply_detail_fetches(
        &items,
        &timeline,
        &mut fetched_reply_details,
        &mut side_tasks,
    );

    let mut key_stream = std::pin::pin!(
        client
            .encryption()
            .backups()
            .room_keys_for_room_stream(&room_id_parsed)
    );
    spawn_backup_key_download(&mut side_tasks, client, &room_id_parsed);

    loop {
        tokio::select! {
            biased;
            diffs = stream.next() => {
                let Some(diffs) = diffs else { break };
                if let Some(patch) = process_diffs(&mut items, diffs, &ctx)
                    && timeline_tx.send(TimelineUpdate::Patch(patch)).is_err()
                {
                    return Ok(());
                }
                spawn_reply_detail_fetches(&items, &timeline, &mut fetched_reply_details, &mut side_tasks);
            }
            result = key_stream.next() => {
                if let Some(Ok(keys)) = result {
                    handle_room_keys(&timeline, keys).await;
                }
            }
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { break };
                handle_timeline_command(cmd, &timeline, &timeline_tx).await;
            }
        }
    }

    Ok(())
}
