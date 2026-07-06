use std::future::Future;

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

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
        self.token.cancel();
        self.tasks.abort_all();
        while self.tasks.join_next().await.is_some() {}
        self.token = CancellationToken::new();
    }

    pub(super) async fn shutdown(&mut self) {
        self.token.cancel();
        self.tasks.abort_all();
        while self.tasks.join_next().await.is_some() {}
    }

    pub(super) fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub(super) fn spawn(&mut self, future: impl Future<Output = ()> + Send + 'static) {
        self.tasks.spawn(future);
    }
}
