use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::{Instant, sleep};
use tokio_util::sync::CancellationToken;

use super::RoomDirectory;
use crate::app::event::{AppEvent, SessionEvent};
use crate::app::input::EventSender;
use crate::app::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::sync::DirectoryUpdate;
use crate::commands::view::{AppViewState, Toast};
use crate::domain::sync::{ConnectionStatus, SyncEvent, SyncOutcome};
use crate::ports::matrix::{SyncPort, SyncSink};
use crate::ports::output::AppOutputPort;

const BACKOFF_START: Duration = Duration::from_secs(1);

const BACKOFF_MAX: Duration = Duration::from_mins(1);

const BACKOFF_RESET_AFTER: Duration = Duration::from_mins(1);

impl RoomDirectory {
    pub(in crate::app) fn spawn_sync_pipeline(
        group: &mut TaskGroup,
        sync: Arc<dyn SyncPort>,
        output: Arc<dyn AppOutputPort>,
        events: EventSender,
        dir_in_tx: mpsc::UnboundedSender<DirectoryUpdate>,
    ) {
        let token = group.token();
        let sink_output = Arc::clone(&output);
        let on_sync: SyncSink = Arc::new(move |event| match event {
            SyncEvent::Connected => {
                sink_output.publish(Box::new(|view| {
                    view.connection = ConnectionStatus::Connected;
                }));
            }
            SyncEvent::Rooms(rooms) => {
                drop(dir_in_tx.send(DirectoryUpdate::Rooms(rooms)));
            }
            SyncEvent::Invites(invites) => {
                drop(dir_in_tx.send(DirectoryUpdate::Invites(invites)));
            }
            SyncEvent::Removals(removals) => {
                drop(dir_in_tx.send(DirectoryUpdate::Removals(removals)));
            }
            SyncEvent::Spaces(spaces) => {
                drop(dir_in_tx.send(DirectoryUpdate::Spaces(spaces)));
            }
            SyncEvent::ConnectionError(msg) => {
                sink_output.publish(Box::new(move |view| {
                    view.connection = ConnectionStatus::Error(msg);
                }));
            }
            SyncEvent::SlidingSyncUnsupported => {
                sink_output.publish(Box::new(explain_missing_sliding_sync));
            }
        });

        group.spawn(supervise_sync(sync, output, events, on_sync, token));
    }
}

fn publish_connection(output: &Arc<dyn AppOutputPort>, status: ConnectionStatus) {
    output.publish(Box::new(move |view| view.connection = status));
}

fn explain_missing_sliding_sync(view: &mut AppViewState) {
    if view.connection != ConnectionStatus::SlidingSyncUnsupported {
        view.toast = Toast::Error(UserMessage::new(UserMessageKind::SlidingSyncUnsupported));
    }
    view.connection = ConnectionStatus::SlidingSyncUnsupported;
}

async fn supervise_sync(
    sync: Arc<dyn SyncPort>,
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    on_sync: SyncSink,
    token: CancellationToken,
) {
    let mut backoff = BACKOFF_START;
    loop {
        let started = Instant::now();
        match sync.start_sync(Arc::clone(&on_sync), token.clone()).await {
            SyncOutcome::Cancelled => return,
            SyncOutcome::SessionLost(loss) => {
                drop(events.send(AppEvent::Session(SessionEvent::from(loss))));
                return;
            }
            SyncOutcome::Fatal(msg) => {
                tracing::error!("sync failed unrecoverably: {msg}");
                publish_connection(&output, ConnectionStatus::Error(msg));
                return;
            }
            SyncOutcome::Recoverable(msg) => {
                if started.elapsed() >= BACKOFF_RESET_AFTER {
                    backoff = BACKOFF_START;
                }
                tracing::warn!("sync ended, retrying in {backoff:?}: {msg}");
                publish_connection(&output, ConnectionStatus::Error(msg));
                tokio::select! {
                    () = token.cancelled() => return,
                    () = sleep(backoff) => {}
                }
                backoff = backoff.saturating_mul(2).min(BACKOFF_MAX);
            }
        }
    }
}
