use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use tokio_util::sync::CancellationToken;

use crate::app::event::{AppEvent, CallEvent};
use crate::app::input::EventSender;
use crate::commands::view::PreviewTrouble;
use crate::domain::call::devices::DeviceChoice;
use crate::domain::call::video::{FeedId, PreviewFailure};
use crate::ports::call::{CallMediaPort, CameraPreview};

pub(super) struct Preview {
    pub(super) serial: u64,
    pub(super) camera: DeviceChoice,
    pub(super) state: PreviewState,
    stop_tx: oneshot::Sender<()>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PreviewState {
    Starting,
    Showing(FeedId),
    Failed(PreviewTrouble),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) enum PreviewOutcome {
    Shown(FeedId),
    Failed(PreviewTrouble),
}

pub(super) struct PreviewTask {
    pub(super) port: Arc<dyn CallMediaPort>,
    pub(super) camera: DeviceChoice,
    pub(super) turn: Arc<Semaphore>,
    pub(super) events: EventSender,
    pub(super) serial: u64,
}

impl Preview {
    pub(super) fn start(
        task: PreviewTask,
        token: CancellationToken,
    ) -> (Self, impl Future<Output = ()> + Send + 'static) {
        tracing::info!(
            preview = task.serial,
            "previewing the camera before the call"
        );
        let (stop_tx, stopped) = oneshot::channel();
        let preview = Self {
            serial: task.serial,
            camera: task.camera.clone(),
            state: PreviewState::Starting,
            stop_tx,
        };
        (preview, task.run(stopped, token))
    }

    pub(super) fn stop(self) {
        tracing::info!(preview = self.serial, "no longer previewing the camera");
        if self.stop_tx.send(()).is_err() {
            tracing::debug!(
                preview = self.serial,
                "the camera preview had already ended"
            );
        }
    }
}

impl PreviewTask {
    async fn run(self, mut stopped: oneshot::Receiver<()>, token: CancellationToken) {
        let Some(turn) = self.take_turn(&mut stopped, &token).await else {
            return;
        };
        let (lost_tx, lost_rx) = oneshot::channel();
        if let Some(preview) = self.open(lost_tx).await {
            self.show(lost_rx, &mut stopped, &token).await;
            preview.close().await;
            tracing::debug!(
                preview = self.serial,
                "the camera preview let go of the camera"
            );
        }
        drop(turn);
    }

    async fn take_turn(
        &self,
        stopped: &mut oneshot::Receiver<()>,
        token: &CancellationToken,
    ) -> Option<OwnedSemaphorePermit> {
        tokio::select! {
            biased;
            _ = stopped => None,
            () = token.cancelled() => None,
            turn = Arc::clone(&self.turn).acquire_owned() => turn
                .inspect_err(|e| tracing::debug!("the camera preview cannot take its turn: {e}"))
                .ok(),
        }
    }

    async fn open(&self, lost: oneshot::Sender<String>) -> Option<Box<dyn CameraPreview>> {
        match self.port.preview_camera(self.camera.clone(), lost).await {
            Ok(preview) => {
                self.tell(PreviewOutcome::Shown(preview.feed()));
                Some(preview)
            }
            Err(failure) => {
                tracing::warn!("the camera preview did not start: {failure}");
                self.tell(PreviewOutcome::Failed(trouble_of(&failure)));
                None
            }
        }
    }

    async fn show(
        &self,
        lost: oneshot::Receiver<String>,
        stopped: &mut oneshot::Receiver<()>,
        token: &CancellationToken,
    ) {
        tokio::select! {
            _ = stopped => {}
            () = token.cancelled() => {}
            Ok(why) = lost => {
                tracing::warn!("the camera preview stopped: {why}");
                self.tell(PreviewOutcome::Failed(PreviewTrouble::Stopped));
            }
        }
    }

    fn tell(&self, outcome: PreviewOutcome) {
        let event = CallEvent::Preview {
            preview: self.serial,
            outcome,
        };
        if self.events.send(AppEvent::Call(event)).is_err() {
            tracing::debug!("the app stopped before hearing from the camera preview");
        }
    }
}

const fn trouble_of(failure: &PreviewFailure) -> PreviewTrouble {
    match failure {
        PreviewFailure::NoCamera => PreviewTrouble::NoCamera,
        PreviewFailure::NotStarted(_) => PreviewTrouble::NotStarted,
    }
}
