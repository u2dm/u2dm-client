use std::sync::Arc;

use futures_util::Stream;
use matrix_sdk::event_cache::EventCacheError;
use matrix_sdk::paginators::PaginatorError;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::{EventId, OwnedEventId, UserId};
use matrix_sdk::{Client, Room};
use matrix_sdk_ui::eyeball_im::VectorDiff;
use matrix_sdk_ui::timeline::{
    Error as TimelineError, Timeline, TimelineEventFocusThreadMode,
    TimelineFocus as SdkTimelineFocus, TimelineItem,
};
use tokio::sync::mpsc;

use super::lookups::{UnreadBoundary, acknowledged_event, locate_unread};
use super::paginate_backwards;
use crate::adapters::matrix::media::MediaService;
use crate::adapters::matrix::timeline::edits::DiscardedEdits;
use crate::adapters::matrix::timeline::event_filter::timeline_builder;
use crate::adapters::matrix::timeline::poll_ends::EndingPolls;
use crate::adapters::matrix::timeline::rowless_sends::RowlessSendGuard;
use crate::domain::timeline::{PaginationOutcome, TimelineFocus, TimelineUpdate};
use crate::error::{AppError, Result};

const UNREAD_LOOKBACK_BATCHES: usize = 4;

const FOCUS_CONTEXT_EVENTS: u16 = 50;

async fn setup_timeline(
    room: &Room,
    focus: &TimelineFocus,
) -> Result<(Arc<Timeline>, PaginationOutcome)> {
    let Some(target) = focus.target() else {
        let timeline = Arc::new(
            timeline_builder(room)
                .await
                .build()
                .await
                .map_err(AppError::other)?,
        );
        let backwards_outcome = paginate_backwards(&timeline).await;
        return Ok((timeline, backwards_outcome));
    };

    let timeline = Arc::new(build_focused_timeline(room, target).await?);
    Ok((timeline, PaginationOutcome::Completed { hit_end: false }))
}

async fn build_focused_timeline(room: &Room, target: &str) -> Result<Timeline> {
    let target: OwnedEventId = target.try_into().map_err(AppError::other)?;
    timeline_builder(room)
        .await
        .with_focus(SdkTimelineFocus::Event {
            target,
            num_context_events: FOCUS_CONTEXT_EVENTS,
            thread_mode: TimelineEventFocusThreadMode::Automatic {
                hide_threaded_events: false,
            },
        })
        .build()
        .await
        .map_err(|e| focus_failure(&e))
}

fn focus_failure(error: &TimelineError) -> AppError {
    if names_unreachable_event(error) {
        return AppError::FocusedEventUnavailable;
    }
    AppError::other(error)
}

fn names_unreachable_event(error: &TimelineError) -> bool {
    let TimelineError::EventCacheError(EventCacheError::InitialPaginationError(paginator)) = error
    else {
        return false;
    };
    match paginator {
        PaginatorError::EventNotFound(_) => true,
        PaginatorError::SdkError(sdk) => matches!(
            sdk.client_api_error_kind(),
            Some(ErrorKind::Forbidden | ErrorKind::NotFound)
        ),
        PaginatorError::InvalidPreviousState { .. } => false,
    }
}

pub(super) struct InitialView<S> {
    pub(super) items: Vec<Arc<TimelineItem>>,
    pub(super) updates: S,
    pub(super) unread: UnreadBoundary,
}

async fn subscribe_initial_view(
    timeline: &Timeline,
    focus: &TimelineFocus,
    acknowledged: Option<&EventId>,
    own_user_id: Option<&UserId>,
) -> InitialView<impl Stream<Item = Vec<VectorDiff<Arc<TimelineItem>>>> + use<>> {
    let (items, updates) = timeline.subscribe().await;
    let items: Vec<Arc<TimelineItem>> = items.into_iter().collect();
    let unread = match focus {
        TimelineFocus::Latest => UnreadBoundary::CaughtUp,
        TimelineFocus::ReadPosition | TimelineFocus::Event(_) => {
            locate_unread(timeline, &items, acknowledged, own_user_id).await
        }
    };
    InitialView {
        items,
        updates,
        unread,
    }
}

async fn paginate_to_read_boundary(
    timeline: &Timeline,
    focus: &TimelineFocus,
    acknowledged: Option<&EventId>,
    own_user_id: Option<&UserId>,
    outcome: PaginationOutcome,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
) -> (
    InitialView<impl Stream<Item = Vec<VectorDiff<Arc<TimelineItem>>>> + use<>>,
    PaginationOutcome,
) {
    let mut outcome = outcome;
    for batch in 0..UNREAD_LOOKBACK_BATCHES {
        let view = subscribe_initial_view(timeline, focus, acknowledged, own_user_id).await;
        if !view.unread.is_unresolved()
            || !matches!(outcome, PaginationOutcome::Completed { hit_end: false })
        {
            return (view, outcome);
        }
        if batch == 0 {
            drop(timeline_tx.send(TimelineUpdate::ResolvingUnread).await);
        }
        tracing::debug!("the read position is not among the subscribed items, paginating");
        outcome = paginate_backwards(timeline).await;
    }
    let view = subscribe_initial_view(timeline, focus, acknowledged, own_user_id).await;
    (view, outcome)
}

pub(super) struct OpenedTimeline<S> {
    pub(super) sends: RowlessSendGuard,
    pub(super) timeline: Arc<Timeline>,
    pub(super) acknowledged: Option<OwnedEventId>,
    pub(super) view: InitialView<S>,
    pub(super) backwards_outcome: PaginationOutcome,
}

pub(super) async fn open_timeline(
    client: &Client,
    media: &MediaService,
    room: Room,
    ending: &Arc<EndingPolls>,
    discarded: &Arc<DiscardedEdits>,
    focus: &TimelineFocus,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
) -> Result<OpenedTimeline<impl Stream<Item = Vec<VectorDiff<Arc<TimelineItem>>>> + use<>>> {
    let sends = RowlessSendGuard::watch(
        room.clone(),
        Arc::clone(ending),
        Arc::clone(discarded),
        timeline_tx,
    )
    .await;
    let (timeline, backwards_outcome) = setup_timeline(&room, focus).await?;

    media.ensure_dirs().await;

    let acknowledged = if focus.opens_at_read_position() {
        acknowledged_event(&timeline, client.user_id()).await
    } else {
        None
    };
    let (view, backwards_outcome) = paginate_to_read_boundary(
        &timeline,
        focus,
        acknowledged.as_deref(),
        client.user_id(),
        backwards_outcome,
        timeline_tx,
    )
    .await;
    Ok(OpenedTimeline {
        sends,
        timeline,
        acknowledged,
        view,
        backwards_outcome,
    })
}
