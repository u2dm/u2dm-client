use std::iter;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::domain::timeline::TimelineCommand;

pub(super) struct Commands {
    rx: mpsc::UnboundedReceiver<TimelineCommand>,
    close: CancellationToken,
}

impl Commands {
    pub(super) fn new(
        rx: mpsc::UnboundedReceiver<TimelineCommand>,
        close: CancellationToken,
    ) -> Self {
        Self { rx, close }
    }

    pub(super) async fn recv(&mut self) -> Option<TimelineCommand> {
        tokio::select! {
            biased;
            () = self.close.cancelled() => None,
            command = self.rx.recv() => command,
        }
    }

    pub(super) async fn closed(&self) {
        self.close.cancelled().await;
    }

    pub(super) fn close_and_take_event_sends(
        &mut self,
    ) -> impl Iterator<Item = TimelineCommand> + '_ {
        self.rx.close();
        iter::from_fn(|| self.rx.try_recv().ok()).filter(TimelineCommand::sends_an_event)
    }
}
