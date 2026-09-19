use std::time::{Duration, SystemTime};

use matrix_sdk::Client;
use matrix_sdk::ruma::api::error::{ErrorKind, RetryAfter};
use matrix_sdk::send_queue::SendQueueRoomError;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

const RESUME_BACKOFF_START: Duration = Duration::from_secs(1);
const RESUME_BACKOFF_MAX: Duration = Duration::from_secs(30);
const RESUME_HEALTHY_AFTER: Duration = Duration::from_mins(1);
const RETRY_AFTER_MAX: Duration = Duration::from_mins(5);

enum Queues {
    Flowing { since: Instant },
    Paused { until: Instant },
}

pub(super) struct SendQueueRecovery {
    errors: Receiver<SendQueueRoomError>,
    queues: Queues,
    backoff: Duration,
}

fn retry_after(error: &matrix_sdk::Error) -> Option<Duration> {
    let Some(ErrorKind::LimitExceeded(limit)) = error.client_api_error_kind() else {
        return None;
    };
    let wait = match limit.retry_after? {
        RetryAfter::Delay(delay) => delay,
        RetryAfter::DateTime(at) => at.duration_since(SystemTime::now()).ok()?,
    };
    Some(wait.min(RETRY_AFTER_MAX))
}

impl SendQueueRecovery {
    pub(super) async fn start(client: &Client) -> Self {
        let errors = client.send_queue().subscribe_errors();
        client.send_queue().set_enabled(true).await;
        Self {
            errors,
            queues: Queues::Flowing {
                since: Instant::now(),
            },
            backoff: RESUME_BACKOFF_START,
        }
    }

    pub(super) async fn next_error(&mut self) -> Result<SendQueueRoomError, RecvError> {
        self.errors.recv().await
    }

    pub(super) fn resume_at(&self) -> Option<Instant> {
        match self.queues {
            Queues::Paused { until } => Some(until),
            Queues::Flowing { .. } => None,
        }
    }

    pub(super) fn on_error(&mut self, failure: &SendQueueRoomError) {
        if failure.is_recoverable {
            let delay = self.pause(retry_after(&failure.error));
            tracing::warn!(
                room_id = %failure.room_id,
                "send queue paused, resuming in {delay:?}: {}",
                failure.error
            );
        } else {
            tracing::warn!(
                room_id = %failure.room_id,
                "send wedged until it is retried or discarded: {}",
                failure.error
            );
        }
    }

    pub(super) fn on_lagged(&mut self, missed: u64) {
        let delay = self.pause(None);
        tracing::warn!("missed {missed} send queue errors, resuming every queue in {delay:?}");
    }

    pub(super) async fn resume(&mut self, client: &Client) {
        if let Queues::Paused { .. } = self.queues {
            self.queues = Queues::Flowing {
                since: Instant::now(),
            };
            tracing::info!("resuming send queues");
            client.send_queue().set_enabled(true).await;
        }
    }

    fn pause(&mut self, retry_after: Option<Duration>) -> Duration {
        let now = Instant::now();
        let scheduled = match self.queues {
            Queues::Paused { until } => until,
            Queues::Flowing { since } => {
                if now.duration_since(since) >= RESUME_HEALTHY_AFTER {
                    self.backoff = RESUME_BACKOFF_START;
                }
                let until = now + self.backoff;
                self.backoff = self.backoff.saturating_mul(2).min(RESUME_BACKOFF_MAX);
                until
            }
        };
        let until = retry_after.map_or(scheduled, |wait| scheduled.max(now + wait));
        self.queues = Queues::Paused { until };
        until.saturating_duration_since(now)
    }
}
