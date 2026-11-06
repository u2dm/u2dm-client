use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::send;
use crate::app::event::SessionEvent;
use crate::app::input::EventSender;
use crate::domain::auth::Session;
use crate::ports::matrix::SessionPort;
use crate::ports::storage::StoragePort;

const PERSIST_RETRY_FLOOR: Duration = Duration::from_secs(1);

const PERSIST_RETRY_CEILING: Duration = Duration::from_mins(1);

pub(super) struct SessionPersister {
    storage: Arc<dyn StoragePort>,
    events: EventSender,
    session: u64,
    retry_in: Duration,
    reported: bool,
}

impl SessionPersister {
    pub(super) fn new(storage: Arc<dyn StoragePort>, events: EventSender, session: u64) -> Self {
        Self {
            storage,
            events,
            session,
            retry_in: PERSIST_RETRY_FLOOR,
            reported: false,
        }
    }

    pub(super) async fn run(mut self, mut session_rx: mpsc::UnboundedReceiver<Session>) {
        let mut dirty: Option<Session> = None;
        let mut retry_in = None;

        loop {
            match next_persist_step(&mut session_rx, retry_in).await {
                PersistStep::Refreshed(session) => dirty = Some(session),
                PersistStep::Retry => {}
                PersistStep::Closed => break,
            }
            let Some(session) = dirty.as_ref() else {
                continue;
            };
            retry_in = self.store(session).await;
            if retry_in.is_none() {
                dirty = None;
            } else {
                self.report_failure_once();
            }
        }

        if let Some(session) = dirty {
            self.flush(&session).await;
        }
    }

    async fn store(&mut self, session: &Session) -> Option<Duration> {
        match self.storage.save_session(session).await {
            Ok(()) => {
                if self.reported {
                    tracing::info!("persisted the session tokens after earlier failures");
                } else {
                    tracing::info!("persisted the current session tokens");
                }
                self.retry_in = PERSIST_RETRY_FLOOR;
                self.reported = false;
                None
            }
            Err(e) => {
                let retry_in = self.retry_in;
                self.retry_in = retry_in.saturating_mul(2).min(PERSIST_RETRY_CEILING);
                tracing::warn!(
                    retry_in = retry_in.as_secs(),
                    "failed to persist refreshed session, retrying: {e}"
                );
                Some(retry_in)
            }
        }
    }

    fn report_failure_once(&mut self) {
        if self.reported {
            return;
        }
        self.reported = true;
        send(
            &self.events,
            SessionEvent::TokensNotPersisted {
                session: self.session,
            },
        );
    }

    async fn flush(&mut self, session: &Session) {
        if self.store(session).await.is_some() {
            tracing::warn!("the newest session tokens were not persisted before shutting down");
        }
    }
}

enum PersistStep {
    Refreshed(Session),
    Retry,
    Closed,
}

pub(super) async fn listen_for_session_changes(
    port: Arc<dyn SessionPort>,
    session_tx: mpsc::UnboundedSender<Session>,
    token: CancellationToken,
) {
    let listened = token
        .run_until_cancelled(port.subscribe_session_changes(session_tx))
        .await;
    match listened {
        Some(Ok(())) => tracing::debug!("session change listener ended"),
        Some(Err(e)) => tracing::warn!("session change listener failed: {e}"),
        None => tracing::debug!("session change listener cancelled"),
    }
}

async fn next_persist_step(
    session_rx: &mut mpsc::UnboundedReceiver<Session>,
    retry_in: Option<Duration>,
) -> PersistStep {
    let Some(delay) = retry_in else {
        return recv_newest_session(session_rx).await;
    };
    tokio::select! {
        biased;
        step = recv_newest_session(session_rx) => step,
        () = sleep(delay) => PersistStep::Retry,
    }
}

async fn recv_newest_session(session_rx: &mut mpsc::UnboundedReceiver<Session>) -> PersistStep {
    let Some(mut newest) = session_rx.recv().await else {
        return PersistStep::Closed;
    };
    while let Ok(newer) = session_rx.try_recv() {
        newest = newer;
    }
    PersistStep::Refreshed(newest)
}
