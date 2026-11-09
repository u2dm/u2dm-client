use std::sync::Arc;

use matrix_sdk::room::Receipts;
use matrix_sdk::ruma::OwnedEventId;
use matrix_sdk_ui::timeline::Timeline;
use tokio::sync::watch;
use tokio_util::task::AbortOnDropHandle;
use tracing::Instrument;

use crate::domain::settings::ReceiptVisibility;

#[derive(Clone)]
struct ReceiptTarget {
    event_id: OwnedEventId,
    visibility: ReceiptVisibility,
}

pub(in crate::adapters::matrix) struct ReceiptLane {
    latest_target: watch::Sender<Option<ReceiptTarget>>,
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

    pub(super) async fn mark_read(&self, timeline: &Timeline, visibility: ReceiptVisibility) {
        if let Some(event_id) = timeline.latest_event_id().await {
            self.latest_target.send_replace(Some(ReceiptTarget {
                event_id,
                visibility,
            }));
        }
    }

    pub(super) fn abandon(&self) {
        self.task.abort();
    }
}

pub(in crate::adapters::matrix) fn read_up_to(
    event_id: OwnedEventId,
    visibility: ReceiptVisibility,
) -> Receipts {
    let receipts = Receipts::new().fully_read_marker(event_id.clone());
    match visibility {
        ReceiptVisibility::Public => receipts.public_read_receipt(event_id),
        ReceiptVisibility::Private => receipts.private_read_receipt(event_id),
    }
}

async fn send_in_order(
    timeline: Arc<Timeline>,
    mut targets: watch::Receiver<Option<ReceiptTarget>>,
) {
    while targets.changed().await.is_ok() {
        let target = targets.borrow_and_update().clone();
        if let Some(target) = target {
            send_receipts(&timeline, target).await;
        }
    }
}

async fn send_receipts(timeline: &Timeline, target: ReceiptTarget) {
    let ReceiptTarget {
        event_id,
        visibility,
    } = target;
    let receipts = read_up_to(event_id.clone(), visibility);
    match timeline.send_multiple_receipts(receipts).await {
        Ok(()) => tracing::debug!(%event_id, "sent the read receipt"),
        Err(e) => tracing::warn!(%event_id, "failed to mark the room as read: {e}"),
    }
}
