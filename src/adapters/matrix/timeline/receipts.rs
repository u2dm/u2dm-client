use std::sync::Arc;

use matrix_sdk::room::Receipts;
use matrix_sdk::ruma::OwnedEventId;
use matrix_sdk_ui::timeline::Timeline;
use tokio::sync::watch;
use tokio_util::task::AbortOnDropHandle;
use tracing::Instrument;

pub(in crate::adapters::matrix) struct ReceiptLane {
    latest_target: watch::Sender<Option<OwnedEventId>>,
    task: AbortOnDropHandle<()>,
}

impl ReceiptLane {
    pub(super) fn spawn(timeline: &Arc<Timeline>) -> Self {
        let (latest_target, targets) = watch::channel(None);
        let task = tokio::spawn(send_in_order(Arc::clone(timeline), targets).in_current_span());
        Self {
            latest_target,
            task: AbortOnDropHandle::new(task),
        }
    }

    pub(super) async fn mark_read(&self, timeline: &Timeline) {
        if let Some(event_id) = timeline.latest_event_id().await {
            self.latest_target.send_replace(Some(event_id));
        }
    }

    pub(super) fn abandon(&self) {
        self.task.abort();
    }
}

async fn send_in_order(
    timeline: Arc<Timeline>,
    mut targets: watch::Receiver<Option<OwnedEventId>>,
) {
    while targets.changed().await.is_ok() {
        let target = targets.borrow_and_update().clone();
        if let Some(event_id) = target {
            send_receipts(&timeline, event_id).await;
        }
    }
}

async fn send_receipts(timeline: &Timeline, event_id: OwnedEventId) {
    let receipts = Receipts::new()
        .public_read_receipt(event_id.clone())
        .fully_read_marker(event_id.clone());
    match timeline.send_multiple_receipts(receipts).await {
        Ok(()) => tracing::debug!(%event_id, "sent the read receipt"),
        Err(e) => tracing::warn!(%event_id, "failed to mark the room as read: {e}"),
    }
}
