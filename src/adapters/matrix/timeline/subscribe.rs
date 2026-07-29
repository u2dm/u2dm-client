use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};

use futures_util::{Stream, StreamExt};
use matrix_sdk::Client;
use matrix_sdk::room::Receipts;
use matrix_sdk::ruma::events::fully_read::FullyReadEventContent;
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::ruma::{
    EventId, IdParseError, MilliSecondsSinceUnixEpoch, OwnedEventId, OwnedRoomId, UserId,
};
use matrix_sdk_ui::eyeball_im::VectorDiff;
use matrix_sdk_ui::timeline::{
    EventTimelineItem, RoomExt as _, Timeline, TimelineItem, VirtualTimelineItem,
};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use super::diff::diff_to_patch;
use super::filter::TimelineItems;
use super::{EnrichmentPool, TimelineContext};
use crate::adapters::matrix::media::MediaService;
use crate::adapters::matrix::profile::PronounCache;
use crate::domain::models::{
    EnrichmentDelta, PaginationDirection, PaginationOutcome, RoomId, TimelineCommand,
    TimelineMessage, TimelinePatch, TimelineUpdate,
};
use crate::domain::viewport::PAGINATION_BATCH_SIZE;
use crate::error::{AppError, Result};

const REPLY_FETCH_INFLIGHT: usize = 4;
const UNREAD_LOOKBACK_BATCHES: usize = 4;

fn needs_pronouns(msg: &TimelineMessage, pronouns: &PronounCache) -> bool {
    !msg.is_own && pronouns.needs_fetch(&msg.sender)
}

fn spawn_enrichment(ctx: &TimelineContext<'_>, msg: &TimelineMessage) {
    let unique_id = msg.unique_id.clone();
    if let Ok(mut inflight) = ctx.enrich.inflight.lock() {
        if !inflight.insert(unique_id.clone()) {
            return;
        }
    } else {
        return;
    }

    let msg = msg.clone();
    let resolve_pronouns = needs_pronouns(&msg, ctx.pronouns);
    let client = ctx.client.clone();
    let media = Arc::clone(ctx.media);
    let media_sources = Arc::clone(ctx.media_sources);
    let pronouns = Arc::clone(ctx.pronouns);
    let inflight = Arc::clone(&ctx.enrich.inflight);
    let semaphore = Arc::clone(&ctx.enrich.semaphore);
    let token = ctx.enrich.token.clone();
    let tx = ctx.timeline_tx.clone();

    ctx.enrich.tracker.spawn(async move {
        let work = async {
            let _permit = semaphore.acquire().await.ok()?;
            let (thumbnail, avatar_mxc, pronouns) = tokio::join!(
                media.enrich_thumbnail(&client, &media_sources, &msg),
                media.enrich_avatar(&client, &msg),
                async {
                    if resolve_pronouns {
                        let resolved = pronouns.resolve(&client, &msg.sender).await;
                        (!resolved.is_empty()).then_some(resolved)
                    } else {
                        None
                    }
                },
            );
            Some(EnrichmentDelta {
                unique_id: msg.unique_id.clone(),
                event_id: msg.event_id.clone(),
                thumbnail,
                avatar_mxc,
                pronouns,
            })
        };

        let delta = token.run_until_cancelled(work).await.flatten();

        if let Ok(mut inflight) = inflight.lock() {
            inflight.remove(&unique_id);
        }

        if let Some(delta) = delta
            && !delta.is_noop()
        {
            drop(
                tx.send(TimelineUpdate::Patch(Box::new(TimelinePatch::Enrich(
                    delta,
                ))))
                .await,
            );
        }
    });
}

pub(super) fn spawn_enrichment_for_messages(
    messages: &[TimelineMessage],
    ctx: &TimelineContext<'_>,
) {
    for msg in messages {
        if ctx.media.needs_media_download(msg) || needs_pronouns(msg, ctx.pronouns) {
            spawn_enrichment(ctx, msg);
        }
    }
}

async fn send_initial_timeline(
    initial_items: Vec<Arc<TimelineItem>>,
    backwards_outcome: PaginationOutcome,
    ctx: &TimelineContext<'_>,
    room_id: &RoomId,
) -> Option<TimelineItems> {
    let raw_items = initial_items.len();
    let (items, messages) = TimelineItems::load(initial_items, ctx);
    tracing::info!(
        raw_items,
        messages = messages.len(),
        %room_id,
        "timeline loaded"
    );
    spawn_enrichment_for_messages(&messages, ctx);
    ctx.timeline_tx
        .send(TimelineUpdate::Patch(Box::new(TimelinePatch::Reset(
            messages,
        ))))
        .await
        .ok()?;
    ctx.timeline_tx
        .send(TimelineUpdate::Pagination {
            direction: PaginationDirection::Backwards,
            outcome: backwards_outcome,
        })
        .await
        .ok()?;
    Some(items)
}

fn process_diffs(
    items: &mut TimelineItems,
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

async fn handle_timeline_command(
    cmd: TimelineCommand,
    timeline: &Timeline,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
) {
    let (direction, outcome) = match cmd {
        TimelineCommand::PaginateBackwards => (
            PaginationDirection::Backwards,
            paginate_backwards(timeline).await,
        ),
        TimelineCommand::PaginateForwards => (
            PaginationDirection::Forwards,
            paginate_forwards(timeline).await,
        ),
        TimelineCommand::MarkRead => {
            mark_read(timeline).await;
            return;
        }
    };

    if timeline_tx
        .send(TimelineUpdate::Pagination { direction, outcome })
        .await
        .is_err()
    {
        tracing::debug!("timeline update channel closed");
    }
}

async fn mark_read(timeline: &Timeline) {
    let Some(event_id) = timeline.latest_event_id().await else {
        return;
    };
    let receipts = Receipts::new()
        .public_read_receipt(event_id.clone())
        .fully_read_marker(event_id);
    if let Err(e) = timeline.send_multiple_receipts(receipts).await {
        tracing::warn!("failed to mark the room as read: {e}");
    }
}

async fn paginate_backwards(timeline: &Timeline) -> PaginationOutcome {
    tracing::debug!("paginating backwards");
    match timeline.paginate_backwards(PAGINATION_BATCH_SIZE).await {
        Ok(hit_start) => PaginationOutcome::Completed { hit_end: hit_start },
        Err(e) => {
            tracing::warn!("backward pagination failed: {e}");
            PaginationOutcome::Failed
        }
    }
}

async fn paginate_forwards(timeline: &Timeline) -> PaginationOutcome {
    tracing::debug!("paginating forwards");
    match timeline.paginate_forwards(PAGINATION_BATCH_SIZE).await {
        Ok(hit_end) => PaginationOutcome::Completed { hit_end },
        Err(e) => {
            tracing::warn!("forward pagination failed: {e}");
            PaginationOutcome::Failed
        }
    }
}

async fn setup_timeline(
    client: &Client,
    room_id: &RoomId,
) -> Result<(Arc<Timeline>, OwnedRoomId, PaginationOutcome)> {
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

    let backwards_outcome = paginate_backwards(&timeline).await;

    Ok((timeline, room_id_parsed, backwards_outcome))
}

fn spawn_reply_detail_fetches(
    items: &[Arc<TimelineItem>],
    timeline: &Arc<Timeline>,
    fetched: &mut HashSet<String>,
    reply_limit: &Arc<Semaphore>,
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
        let reply_limit = Arc::clone(reply_limit);
        side_tasks.spawn(async move {
            let Ok(_permit) = reply_limit.acquire().await else {
                return;
            };
            let Ok(id) = OwnedEventId::try_from(event_id.as_str()) else {
                return;
            };
            if let Err(e) = timeline.fetch_details_for_event(&id).await {
                tracing::debug!("failed to fetch reply details: {e}");
            }
        });
    }
}

async fn fully_read_marker(timeline: &Timeline) -> Option<OwnedEventId> {
    match timeline
        .room()
        .account_data_static::<FullyReadEventContent>()
        .await
    {
        Ok(Some(raw)) => raw.deserialize().ok().map(|event| event.content.event_id),
        Ok(None) => None,
        Err(e) => {
            tracing::debug!("the fully-read marker could not be read: {e}");
            None
        }
    }
}

#[derive(Debug)]
struct ReadPosition {
    event_id: OwnedEventId,
    sent_at: Option<MilliSecondsSinceUnixEpoch>,
}

async fn read_boundary(timeline: &Timeline, own_user_id: Option<&UserId>) -> Option<ReadPosition> {
    if let Some(user_id) = own_user_id
        && let Some((event_id, receipt)) = timeline.latest_user_read_receipt(user_id).await
    {
        return Some(ReadPosition {
            event_id,
            sent_at: receipt.ts,
        });
    }
    fully_read_marker(timeline)
        .await
        .map(|event_id| ReadPosition {
            event_id,
            sent_at: None,
        })
}

fn event_id_of(item: &TimelineItem) -> Option<&EventId> {
    item.as_event().and_then(EventTimelineItem::event_id)
}

fn contains_event(items: &[Arc<TimelineItem>], event_id: &EventId) -> bool {
    items.iter().any(|item| event_id_of(item) == Some(event_id))
}

fn first_unread_index(
    items: &[Arc<TimelineItem>],
    boundary: Option<&ReadPosition>,
) -> Option<usize> {
    let after_marker = items
        .iter()
        .rposition(|item| matches!(item.as_virtual(), Some(VirtualTimelineItem::ReadMarker)))
        .map(|marker| marker.saturating_add(1));
    let after_boundary = boundary.and_then(|boundary| first_unread_after(items, boundary));
    match (after_marker, after_boundary) {
        (Some(marker), Some(boundary)) => Some(marker.max(boundary)),
        (marker, boundary) => marker.or(boundary),
    }
}

fn first_unread_after(items: &[Arc<TimelineItem>], boundary: &ReadPosition) -> Option<usize> {
    row_after_event(items, &boundary.event_id).or_else(|| {
        boundary
            .sent_at
            .map(|sent_at| row_after_timestamp(items, sent_at))
    })
}

fn row_after_event(items: &[Arc<TimelineItem>], event_id: &EventId) -> Option<usize> {
    items
        .iter()
        .rposition(|item| event_id_of(item) == Some(event_id))
        .map(|index| index.saturating_add(1))
}

fn row_after_timestamp(items: &[Arc<TimelineItem>], sent_at: MilliSecondsSinceUnixEpoch) -> usize {
    items
        .iter()
        .rposition(|item| {
            item.as_event()
                .is_some_and(|event| event.timestamp() <= sent_at)
        })
        .map_or(0, |index| index.saturating_add(1))
}

fn first_unread_event_id(
    items: &[Arc<TimelineItem>],
    boundary: Option<&ReadPosition>,
    own_user_id: Option<&UserId>,
) -> Option<String> {
    let first_unread = first_unread_index(items, boundary)?;
    items
        .get(first_unread..)?
        .iter()
        .filter_map(|item| item.as_event())
        .find(|event| own_user_id != Some(event.sender()))
        .and_then(|event| event.event_id().map(ToString::to_string))
}

async fn paginate_to_read_boundary(
    timeline: &Timeline,
    boundary: &ReadPosition,
    outcome: PaginationOutcome,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
) -> PaginationOutcome {
    let mut outcome = outcome;
    let mut announced = false;
    for _ in 0..UNREAD_LOOKBACK_BATCHES {
        if !matches!(outcome, PaginationOutcome::Completed { hit_end: false }) {
            return outcome;
        }
        let items: Vec<Arc<TimelineItem>> = timeline.items().await.into_iter().collect();
        if contains_event(&items, &boundary.event_id) {
            return outcome;
        }
        if !announced {
            announced = true;
            drop(timeline_tx.send(TimelineUpdate::ResolvingUnread).await);
        }
        tracing::debug!("the read position is older than the loaded timeline, paginating");
        outcome = paginate_backwards(timeline).await;
    }
    outcome
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
    media: &Arc<MediaService>,
    media_sources: &Arc<StdMutex<HashMap<String, MediaSource>>>,
    pronouns: &Arc<PronounCache>,
    room_id: &RoomId,
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    mut cmd_rx: mpsc::UnboundedReceiver<TimelineCommand>,
) -> Result<()> {
    let (timeline, room_id_parsed, backwards_outcome) = setup_timeline(client, room_id).await?;

    if let Ok(mut sources) = media_sources.lock() {
        sources.clear();
    }

    media.ensure_dirs().await;

    let boundary = read_boundary(&timeline, client.user_id()).await;
    let backwards_outcome = match boundary.as_ref() {
        Some(boundary) => {
            paginate_to_read_boundary(&timeline, boundary, backwards_outcome, &timeline_tx).await
        }
        None => backwards_outcome,
    };

    let (initial_items, stream) = timeline.subscribe().await;

    let mut side_tasks = JoinSet::new();
    side_tasks.spawn({
        let timeline = Arc::clone(&timeline);
        async move { timeline.fetch_members().await }
    });

    let initial_items: Vec<Arc<TimelineItem>> = initial_items.into_iter().collect();
    let first_unread = first_unread_event_id(&initial_items, boundary.as_ref(), client.user_id());
    tracing::debug!(
        ?boundary,
        ?first_unread,
        items = initial_items.len(),
        %room_id,
        "resolved the read position"
    );

    let own_user_id = client.user_id().map(ToString::to_string);
    let enrich = EnrichmentPool::new();
    let ctx = TimelineContext {
        client,
        media,
        media_sources,
        pronouns,
        own_user_id: own_user_id.as_deref(),
        first_unread: first_unread.as_deref(),
        timeline_tx: &timeline_tx,
        enrich: &enrich,
    };

    let Some(items) = send_initial_timeline(initial_items, backwards_outcome, &ctx, room_id).await
    else {
        return Ok(());
    };

    run_timeline_loop(
        &ctx,
        &timeline,
        &room_id_parsed,
        items,
        side_tasks,
        &mut cmd_rx,
        stream,
    )
    .await;

    Ok(())
}

async fn run_timeline_loop<S>(
    ctx: &TimelineContext<'_>,
    timeline: &Arc<Timeline>,
    room_id_parsed: &OwnedRoomId,
    mut items: TimelineItems,
    mut side_tasks: JoinSet<()>,
    cmd_rx: &mut mpsc::UnboundedReceiver<TimelineCommand>,
    mut stream: S,
) where
    S: Stream<Item = Vec<VectorDiff<Arc<TimelineItem>>>> + Unpin,
{
    let mut fetched_reply_details: HashSet<String> = HashSet::new();
    let reply_limit = Arc::new(Semaphore::new(REPLY_FETCH_INFLIGHT));
    spawn_reply_detail_fetches(
        items.items(),
        timeline,
        &mut fetched_reply_details,
        &reply_limit,
        &mut side_tasks,
    );

    let mut key_stream = std::pin::pin!(
        ctx.client
            .encryption()
            .backups()
            .room_keys_for_room_stream(room_id_parsed)
    );
    spawn_backup_key_download(&mut side_tasks, ctx.client, room_id_parsed);

    let mut key_stream_done = false;

    loop {
        tokio::select! {
            biased;
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { break };
                handle_timeline_command(cmd, timeline, ctx.timeline_tx).await;
            }
            result = key_stream.next(), if !key_stream_done => {
                match result {
                    Some(Ok(keys)) => handle_room_keys(timeline, keys).await,
                    Some(Err(error)) => tracing::warn!(%error, "room key stream lagged"),
                    None => key_stream_done = true,
                }
            }
            Some(_) = side_tasks.join_next(), if !side_tasks.is_empty() => {}
            diffs = stream.next() => {
                let Some(diffs) = diffs else { break };
                if let Some(patch) = process_diffs(&mut items, diffs, ctx)
                    && ctx.timeline_tx
                        .send(TimelineUpdate::Patch(Box::new(patch)))
                        .await
                        .is_err()
                {
                    break;
                }
                spawn_reply_detail_fetches(items.items(), timeline, &mut fetched_reply_details, &reply_limit, &mut side_tasks);
            }
        }
    }
}
