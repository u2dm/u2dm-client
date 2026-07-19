use std::future::Future;
use std::time::Duration;

use tokio::task::JoinSet;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

pub(super) struct TaskGroup {
    token: CancellationToken,
    tasks: JoinSet<()>,
}

impl TaskGroup {
    pub(super) fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            tasks: JoinSet::new(),
        }
    }

    pub(super) async fn reset(&mut self) {
        self.stop_tasks().await;
        self.token = CancellationToken::new();
    }

    pub(super) async fn shutdown(&mut self) {
        self.stop_tasks().await;
    }

    async fn stop_tasks(&mut self) {
        self.token.cancel();

        let grace = sleep(SHUTDOWN_GRACE);
        tokio::pin!(grace);
        loop {
            tokio::select! {
                biased;
                joined = self.tasks.join_next() => {
                    if joined.is_none() {
                        return;
                    }
                }
                () = &mut grace => break,
            }
        }

        self.tasks.abort_all();
        while self.tasks.join_next().await.is_some() {}
    }

    pub(super) fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub(super) fn spawn(&mut self, future: impl Future<Output = ()> + Send + 'static) {
        while self.tasks.try_join_next().is_some() {}
        self.tasks.spawn(future);
    }
}
