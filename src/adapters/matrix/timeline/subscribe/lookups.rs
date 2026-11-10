use std::sync::Arc;

use matrix_sdk::ruma::events::fully_read::FullyReadEventContent;
use matrix_sdk::ruma::{EventId, OwnedEventId, UserId};
use matrix_sdk_ui::timeline::{EventTimelineItem, Timeline, TimelineItem, VirtualTimelineItem};
use tokio::sync::mpsc;

use crate::adapters::matrix::timeline::convert::renders;
use crate::adapters::matrix::timeline::filter::TimelineItems;
use crate::adapters::matrix::timeline::{TimelineContext, source};
use crate::domain::settings::RowFilter;
use crate::domain::timeline::{AudioLookup, JumpTarget, TimelineUpdate, locate_audio};

pub(super) async fn report_jump(
    event_id: String,
    items: &TimelineItems,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
) {
    let target = OwnedEventId::try_from(event_id.as_str())
        .ok()
        .map_or(JumpTarget::NotLoaded, |id| items.row_of_event(&id));
    tracing::debug!(event_id, ?target, "resolved a jump target");
    drop(
        timeline_tx
            .send(TimelineUpdate::JumpOutcome { event_id, target })
            .await,
    );
}

pub(super) async fn report_audio(
    request: u64,
    lookup: &AudioLookup,
    items: &TimelineItems,
    ctx: &TimelineContext<'_>,
) {
    let anchor = OwnedEventId::try_from(lookup.anchor())
        .ok()
        .and_then(|event_id| items.position_of_event(&event_id));
    let track = anchor.and_then(|start| locate_audio(lookup, items.messages_from(start).cloned()));
    tracing::debug!(request, found = track.is_some(), "resolved an audio lookup");
    drop(
        ctx.timeline_tx
            .send(TimelineUpdate::AudioLocated {
                request,
                track: track.map(Box::new),
            })
            .await,
    );
}

pub(super) async fn report_source(
    request: u64,
    event_id: &str,
    items: &TimelineItems,
    ctx: &TimelineContext<'_>,
) {
    let source = OwnedEventId::try_from(event_id)
        .ok()
        .and_then(|id| items.position_of_event(&id))
        .and_then(|raw| items.items().get(raw))
        .and_then(|item| item.as_event())
        .and_then(source::of);
    tracing::debug!(
        request,
        event_id,
        found = source.is_some(),
        "resolved a source lookup"
    );
    drop(
        ctx.timeline_tx
            .send(TimelineUpdate::SourceLocated {
                request,
                source: source.map(Box::new),
            })
            .await,
    );
}

pub(super) async fn report_readers(
    request: u64,
    event_id: &str,
    items: &TimelineItems,
    ctx: &TimelineContext<'_>,
) {
    let readers = OwnedEventId::try_from(event_id)
        .ok()
        .and_then(|id| items.readers_of(&id, ctx.own_user_id));
    tracing::debug!(
        request,
        event_id,
        readers = readers.as_ref().map(|found| found.readers.len()),
        "resolved a readers lookup"
    );
    drop(
        ctx.timeline_tx
            .send(TimelineUpdate::ReadersLocated {
                request,
                readers: readers.map(Box::new),
            })
            .await,
    );
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
pub(super) enum UnreadBoundary {
    UnreadFrom(usize),
    CaughtUp,
    Unresolved,
}

impl UnreadBoundary {
    pub(super) fn first_unread(
        &self,
        items: &[Arc<TimelineItem>],
        own_user_id: Option<&UserId>,
        rows: RowFilter,
    ) -> Option<String> {
        match self {
            Self::UnreadFrom(raw_index) => {
                first_unread_event_id(items, *raw_index, own_user_id, rows)
            }
            Self::CaughtUp | Self::Unresolved => None,
        }
    }

    pub(super) fn is_unresolved(&self) -> bool {
        matches!(self, Self::Unresolved)
    }
}

pub(super) async fn acknowledged_event(
    timeline: &Timeline,
    own_user_id: Option<&UserId>,
) -> Option<OwnedEventId> {
    if let Some(user_id) = own_user_id
        && let Some((event_id, _)) = timeline.latest_user_read_receipt(user_id).await
    {
        return Some(event_id);
    }
    fully_read_marker(timeline).await
}

fn event_id_of(item: &TimelineItem) -> Option<&EventId> {
    item.as_event().and_then(EventTimelineItem::event_id)
}

pub(super) async fn locate_unread(
    timeline: &Timeline,
    items: &[Arc<TimelineItem>],
    acknowledged: Option<&EventId>,
    own_user_id: Option<&UserId>,
) -> UnreadBoundary {
    let Some(first_unread) = first_unread_index(timeline, items, acknowledged, own_user_id).await
    else {
        return match acknowledged {
            Some(_) => UnreadBoundary::Unresolved,
            None => UnreadBoundary::CaughtUp,
        };
    };
    UnreadBoundary::UnreadFrom(first_unread)
}

async fn first_unread_index(
    timeline: &Timeline,
    items: &[Arc<TimelineItem>],
    acknowledged: Option<&EventId>,
    own_user_id: Option<&UserId>,
) -> Option<usize> {
    let after_marker = row_after_read_marker(items);
    let after_acknowledged =
        row_after_acknowledged(timeline, items, acknowledged, own_user_id).await;
    match (after_marker, after_acknowledged) {
        (Some(marker), Some(acknowledged)) => Some(marker.max(acknowledged)),
        (marker, acknowledged) => marker.or(acknowledged),
    }
}

fn row_after_read_marker(items: &[Arc<TimelineItem>]) -> Option<usize> {
    items
        .iter()
        .rposition(|item| matches!(item.as_virtual(), Some(VirtualTimelineItem::ReadMarker)))
        .map(|marker| marker.saturating_add(1))
}

async fn row_after_acknowledged(
    timeline: &Timeline,
    items: &[Arc<TimelineItem>],
    acknowledged: Option<&EventId>,
    own_user_id: Option<&UserId>,
) -> Option<usize> {
    if let Some(row) = row_after_event(items, acknowledged?) {
        return Some(row);
    }
    let shown_receipt = timeline
        .latest_user_read_receipt_timeline_event_id(own_user_id?)
        .await?;
    row_after_event(items, &shown_receipt)
}

fn row_after_event(items: &[Arc<TimelineItem>], event_id: &EventId) -> Option<usize> {
    items
        .iter()
        .rposition(|item| event_id_of(item) == Some(event_id))
        .map(|index| index.saturating_add(1))
}

fn first_unread_event_id(
    items: &[Arc<TimelineItem>],
    first_unread: usize,
    own_user_id: Option<&UserId>,
    rows: RowFilter,
) -> Option<String> {
    items
        .get(first_unread..)?
        .iter()
        .filter(|item| renders(item, rows))
        .filter_map(|item| item.as_event())
        .find(|event| own_user_id != Some(event.sender()))
        .and_then(|event| event.event_id().map(ToString::to_string))
}
