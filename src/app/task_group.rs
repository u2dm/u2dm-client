use std::future::Future;
use std::time::Duration;

use tokio::task::{JoinError, JoinSet};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

pub(super) struct TaskGroup {
    name: &'static str,
    token: CancellationToken,
    tasks: JoinSet<()>,
}

impl TaskGroup {
    pub(super) fn new(name: &'static str) -> Self {
        Self {
            name,
            token: CancellationToken::new(),
            tasks: JoinSet::new(),
        }
    }

    pub(super) fn cancel_and_detach(&mut self) {
        self.token.cancel();
        self.arm_next_token();
        self.reap_finished();
        if !self.tasks.is_empty() {
            tracing::debug!(
                group = self.name,
                detached = self.tasks.len(),
                "left cancelled tasks to finish unobserved"
            );
        }
    }

    pub(super) async fn restart(&mut self) {
        self.cancel_and_drain().await;
        self.arm_next_token();
    }

    pub(super) async fn shutdown(&mut self) {
        self.cancel_and_drain().await;
    }

    async fn cancel_and_drain(&mut self) {
        self.token.cancel();
        if !self.drain_within_grace().await {
            self.abort_stragglers().await;
        }
    }

    async fn drain_within_grace(&mut self) -> bool {
        let name = self.name;
        let grace = sleep(SHUTDOWN_GRACE);
        tokio::pin!(grace);
        loop {
            tokio::select! {
                biased;
                joined = self.tasks.join_next() => {
                    match joined {
                        Some(result) => record_join(name, result),
                        None => return true,
                    }
                }
                () = &mut grace => return false,
            }
        }
    }

    async fn abort_stragglers(&mut self) {
        let name = self.name;
        tracing::warn!(
            group = name,
            stragglers = self.tasks.len(),
            "tasks outlived the grace period, aborting"
        );
        self.tasks.abort_all();
        while let Some(result) = self.tasks.join_next().await {
            record_join(name, result);
        }
    }

    fn arm_next_token(&mut self) {
        self.token = CancellationToken::new();
    }

    pub(super) fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub(super) fn spawn(&mut self, future: impl Future<Output = ()> + Send + 'static) {
        self.reap_finished();
        self.tasks.spawn(future);
    }

    fn reap_finished(&mut self) {
        let name = self.name;
        while let Some(result) = self.tasks.try_join_next() {
            record_join(name, result);
        }
    }
}

pub(super) fn record_join(group: &str, result: Result<(), JoinError>) {
    let Err(e) = result else { return };
    if e.is_panic() {
        tracing::error!(group, "task panicked: {e}");
    } else {
        tracing::debug!(group, "task cancelled before completion: {e}");
    }
}
