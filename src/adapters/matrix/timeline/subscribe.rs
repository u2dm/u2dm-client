mod enrichment;
mod lookups;
mod opening;
mod reactions;
mod row_fetches;

use std::collections::HashSet;
use std::sync::Arc;

use futures_util::{Stream, StreamExt};
use matrix_sdk::Client;
use matrix_sdk::ruma::EventId;
use matrix_sdk_ui::eyeball_im::VectorDiff;
use matrix_sdk_ui::timeline::{Timeline, TimelineItem};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tracing::Instrument;

use self::enrichment::{REPLY_FETCH_INFLIGHT, spawn_reply_detail_fetches};
pub(super) use self::enrichment::{enrich_message, enrich_messages};
use self::lookups::{report_audio, report_jump, report_readers, report_source};
use self::opening::{InitialView, OpenedTimeline, open_timeline};
use self::reactions::toggle_reaction;
use self::row_fetches::{RowFetches, fetched_patch, send_patch};
use super::commands::Commands;
use super::diff::{diff_to_patch, stamp_editable_polls, stamp_read_marks};
use super::edits::DiscardedEdits;
use super::emoji_files::EmojiFiles;
use super::filter::TimelineItems;
use super::members::Members;
use super::missing_keys::MissingKeys;
use super::poll_ends::EndingPolls;
use super::receipts::ReceiptLane;
use super::rowless_sends::{RowlessSendEvent, RowlessSendGuard};
use super::undecrypted::UndecryptedResponses;
use super::{EnrichmentPool, TimelineContext, polls};
use crate::adapters::matrix::media::MediaService;
use crate::adapters::matrix::profile::PronounCache;
use crate::adapters::matrix::session::known_room;
use crate::domain::room::RoomId;
use crate::domain::timeline::{
    PaginationDirection, PaginationOutcome, TimelineCommand, TimelineFocus, TimelinePatch,
    TimelineUpdate,
};
use crate::domain::viewport::PAGINATION_BATCH_SIZE;
use crate::error::Result;

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
    enrich_messages(items.rendered_from(0), ctx);
    ctx.timeline_tx
        .send(TimelineUpdate::patch(TimelinePatch::Reset(messages)))
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
) -> Option<TimelineUpdate> {
    tracing::debug!(num_diffs = diffs.len(), "processing incoming diffs");
    let earlier = items.items().to_vec();
    let mut batch = Vec::new();
    for diff in diffs {
        if let Some(patch) = diff_to_patch(items, diff, ctx) {
            tracing::debug!(patch = patch.label(), "diff produced patch");
            batch.push(patch);
        }
    }
    let arrivals = items.arrivals_since(&earlier);
    stamp_editable_polls(items, &mut batch, ctx);
    stamp_read_marks(items, &mut batch, ctx);
    let result = bundled(batch);
    tracing::debug!(
        produced = result.is_some(),
        label = result.as_ref().map(TimelinePatch::label),
        new_messages = arrivals.new_messages,
        from_others = arrivals.from_others,
        "process_diffs result"
    );
    result.map(|patch| TimelineUpdate::Patch {
        patch: Box::new(patch),
        arrivals,
    })
}

async fn refresh_row(items: &mut TimelineItems, event_id: &EventId, ctx: &TimelineContext<'_>) {
    let Some(raw_index) = items.position_of_event(event_id) else {
        return;
    };
    let Some(message) = items.reconvert(raw_index, ctx) else {
        return;
    };
    let patch = TimelinePatch::Set {
        index: items.msg_index_at(raw_index),
        message,
    };
    drop(ctx.timeline_tx.send(TimelineUpdate::patch(patch)).await);
}

async fn settle_rowless_send(
    sends: &mut RowlessSendGuard,
    event: RowlessSendEvent,
    items: &mut TimelineItems,
    ctx: &TimelineContext<'_>,
) {
    if let Some(poll) = sends.settle(event, ctx.timeline_tx).await {
        refresh_row(items, &poll, ctx).await;
    }
}

async fn handle_timeline_command(
    cmd: TimelineCommand,
    timeline: &Timeline,
    items: &mut TimelineItems,
    ctx: &TimelineContext<'_>,
    sends: &RowlessSendGuard,
) {
    let timeline_tx = ctx.timeline_tx;
    let (direction, outcome) = match cmd {
        TimelineCommand::PaginateBackwards => (
            PaginationDirection::Backwards,
            paginate_backwards(timeline).await,
        ),
        TimelineCommand::PaginateForwards => (
            PaginationDirection::Forwards,
            paginate_forwards(timeline).await,
        ),
        TimelineCommand::MarkRead(visibility) => {
            ctx.receipts.mark_read(timeline, visibility).await;
            return;
        }
        TimelineCommand::JumpTo(event_id) => {
            report_jump(event_id, items, timeline_tx).await;
            return;
        }
        TimelineCommand::ToggleReaction {
            event_id,
            key,
            shortcode,
        } => {
            toggle_reaction(timeline, &event_id, &key, shortcode.as_deref()).await;
            return;
        }
        TimelineCommand::VotePoll {
            event_id,
            answer_id,
        } => {
            polls::vote(timeline, items, &event_id, &answer_id, timeline_tx).await;
            return;
        }
        TimelineCommand::EndPoll { event_id } => {
            if let Some(poll) = polls::end(timeline, items, &event_id, sends, timeline_tx).await {
                refresh_row(items, &poll, ctx).await;
            }
            return;
        }
        TimelineCommand::EditPoll { event_id, draft } => {
            polls::edit(timeline, items, &event_id, &draft, timeline_tx).await;
            return;
        }
        TimelineCommand::LocateAudio { request, lookup } => {
            report_audio(request, &lookup, items, ctx).await;
            return;
        }
        TimelineCommand::LocateSource { request, event_id } => {
            report_source(request, &event_id, items, ctx).await;
            return;
        }
        TimelineCommand::LocateReaders { request, event_id } => {
            report_readers(request, &event_id, items, ctx).await;
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

async fn run_command(
    command: TimelineCommand,
    commands: &Commands,
    timeline: &Timeline,
    items: &mut TimelineItems,
    ctx: &TimelineContext<'_>,
    sends: &RowlessSendGuard,
) {
    if command.sends_an_event() {
        handle_timeline_command(command, timeline, items, ctx, sends).await;
        return;
    }
    tokio::select! {
        biased;
        () = commands.closed() => {
            tracing::debug!("the timeline closed during a command, abandoning it");
        }
        () = handle_timeline_command(command, timeline, items, ctx, sends) => {}
    }
}

async fn hand_over_event_sends(
    commands: &mut Commands,
    timeline: &Timeline,
    items: &mut TimelineItems,
    ctx: &TimelineContext<'_>,
    sends: &mut RowlessSendGuard,
) {
    let mut handed_over = 0_usize;
    for command in commands.close_and_take_event_sends() {
        handle_timeline_command(command, timeline, items, ctx, sends).await;
        handed_over += 1;
    }
    sends.report_unsent_ends(ctx.timeline_tx).await;
    tracing::debug!(
        handed_over,
        "the closed timeline handed over its queued event sends"
    );
}

async fn paginate_backwards(timeline: &Timeline) -> PaginationOutcome {
    tracing::debug!("paginating backwards");
    match timeline.paginate_backwards(PAGINATION_BATCH_SIZE).await {
        Ok(hit_start) => {
            tracing::debug!(hit_start, "paginated backwards");
            PaginationOutcome::Completed { hit_end: hit_start }
        }
        Err(e) => {
            tracing::warn!("backward pagination failed: {e}");
            PaginationOutcome::Failed
        }
    }
}

async fn paginate_forwards(timeline: &Timeline) -> PaginationOutcome {
    tracing::debug!("paginating forwards");
    match timeline.paginate_forwards(PAGINATION_BATCH_SIZE).await {
        Ok(hit_end) => {
            tracing::debug!(hit_end, "paginated forwards");
            PaginationOutcome::Completed { hit_end }
        }
        Err(e) => {
            tracing::warn!("forward pagination failed: {e}");
            PaginationOutcome::Failed
        }
    }
}

pub(crate) async fn subscribe_timeline(
    client: &Client,
    media: &Arc<MediaService>,
    pronouns: &Arc<PronounCache>,
    room_id: &RoomId,
    focus: &TimelineFocus,
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    commands: Commands,
) -> Result<()> {
    let room = known_room(client, room_id)?;
    let ending = Arc::new(EndingPolls::default());
    let discarded = Arc::new(DiscardedEdits::default());
    let opening = open_timeline(
        client,
        media,
        room,
        &ending,
        &discarded,
        focus,
        &timeline_tx,
    );
    let opened = tokio::select! {
        biased;
        () = commands.closed() => return Ok(()),
        opened = opening => opened?,
    };
    let OpenedTimeline {
        sends,
        timeline,
        acknowledged,
        view,
        backwards_outcome,
    } = opened;
    let InitialView {
        items: initial_items,
        updates: stream,
        unread,
    } = view;

    let mut side_tasks = JoinSet::new();
    side_tasks.spawn({
        let timeline = Arc::clone(&timeline);
        async move { timeline.fetch_members().await }.in_current_span()
    });

    tracing::debug!(
        ?acknowledged,
        ?unread,
        items = initial_items.len(),
        %room_id,
        "resolved the read position"
    );
    if unread.is_unresolved() {
        drop(timeline_tx.send(TimelineUpdate::UnreadUnresolved).await);
    }

    let own_user_id = client.user_id().map(ToString::to_string);
    let enrich = EnrichmentPool::new();
    let receipts = ReceiptLane::spawn(&timeline);
    let members = Arc::new(Members::default());
    let emoji_files = EmojiFiles::default();
    let undecrypted = UndecryptedResponses::default();
    let ctx = TimelineContext {
        client,
        media,
        pronouns,
        members: &members,
        emoji_files: &emoji_files,
        ending: &ending,
        undecrypted: &undecrypted,
        discarded: &discarded,
        own_user_id: own_user_id.as_deref(),
        focused: focus.target().is_some(),
        first_unread: unread.first_unread(),
        timeline_tx: &timeline_tx,
        enrich: &enrich,
        receipts: &receipts,
    };

    let Some(items) = send_initial_timeline(initial_items, backwards_outcome, &ctx, room_id).await
    else {
        return Ok(());
    };

    if let Some(target) = focus.target() {
        report_jump(target.to_owned(), &items, &timeline_tx).await;
    }

    run_timeline_loop(&ctx, &timeline, sends, items, side_tasks, commands, stream).await;

    Ok(())
}

fn bundled(mut patches: Vec<TimelinePatch>) -> Option<TimelinePatch> {
    match patches.len() {
        0 => None,
        1 => patches.pop(),
        _ => Some(TimelinePatch::Batch(patches)),
    }
}

async fn run_timeline_loop<S>(
    ctx: &TimelineContext<'_>,
    timeline: &Arc<Timeline>,
    mut sends: RowlessSendGuard,
    mut items: TimelineItems,
    mut side_tasks: JoinSet<()>,
    mut commands: Commands,
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

    let room = sends.room().clone();
    let mut row_fetches = RowFetches::new();
    row_fetches.spawn(ctx, &room, &mut side_tasks);
    let mut missing_keys = MissingKeys::new(ctx.client, room.room_id().to_owned());

    loop {
        tokio::select! {
            biased;
            command = commands.recv() => {
                let Some(command) = command else { break };
                run_command(command, &commands, timeline, &mut items, ctx, &sends).await;
            }
            Some(_) = side_tasks.join_next(), if !side_tasks.is_empty() => {}
            event = sends.next() => settle_rowless_send(&mut sends, event, &mut items, ctx).await,
            Some(fetched) = row_fetches.next() => {
                if send_patch(ctx, fetched_patch(&mut items, fetched, ctx)).await.is_break() {
                    break;
                }
            }
            from_backup = missing_keys.step(ctx.client, items.items()) => {
                from_backup.retry_decryption(timeline).await;
            }
            diffs = stream.next() => {
                let Some(diffs) = diffs else { break };
                if let Some(update) = process_diffs(&mut items, diffs, ctx)
                    && ctx.timeline_tx.send(update).await.is_err()
                {
                    break;
                }
                spawn_reply_detail_fetches(items.items(), timeline, &mut fetched_reply_details, &reply_limit, &mut side_tasks);
                row_fetches.spawn(ctx, &room, &mut side_tasks);
            }
        }
    }

    ctx.receipts.abandon();
    hand_over_event_sends(&mut commands, timeline, &mut items, ctx, &mut sends).await;
}
