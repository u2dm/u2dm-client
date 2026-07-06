use std::sync::Arc;

use tokio::sync::mpsc;

use super::task_group::TaskGroup;
use crate::domain::models::VerificationEvent;
use crate::ports::matrix::MatrixPort;
use crate::ports::output::AppOutputPort;

pub(super) struct VerificationController {
    matrix: Arc<dyn MatrixPort>,
    output: Arc<dyn AppOutputPort>,
}

impl VerificationController {
    pub(super) fn new(matrix: Arc<dyn MatrixPort>, output: Arc<dyn AppOutputPort>) -> Self {
        Self { matrix, output }
    }

    pub(super) fn spawn_forwarder(&self, group: &mut TaskGroup) {
        let matrix = Arc::clone(&self.matrix);
        let output = Arc::clone(&self.output);
        let token = group.token();
        group.spawn(async move {
            let (verif_tx, mut verif_rx) = mpsc::unbounded_channel::<VerificationEvent>();
            let listen = matrix.listen_for_verification(verif_tx);
            let forward = async {
                while let Some(event) = verif_rx.recv().await {
                    output.verification(event);
                }
            };

            tokio::select! {
                result = listen => {
                    if let Err(e) = result {
                        tracing::warn!("verification listener ended: {e}");
                    }
                }
                () = forward => {
                    tracing::debug!("verification forwarder stopped");
                }
                () = token.cancelled() => {
                    tracing::debug!("verification listener cancelled");
                }
            }
        });
    }

    pub(super) async fn accept(&self) {
        if let Err(e) = self.matrix.accept_verification().await {
            tracing::warn!("verification accept failed: {e}");
            self.notify_error(format!("Verification accept failed: {e}"));
        }
    }

    pub(super) async fn reject(&self) {
        if let Err(e) = self.matrix.reject_verification().await {
            tracing::warn!("verification reject failed: {e}");
            self.notify_error(format!("Verification reject failed: {e}"));
        }
    }

    pub(super) async fn confirm(&self) {
        if let Err(e) = self.matrix.confirm_verification().await {
            tracing::warn!("verification confirm failed: {e}");
            self.notify_error(format!("Verification confirm failed: {e}"));
        }
    }

    fn notify_error(&self, msg: impl Into<String>) {
        self.output.notify_error(msg.into());
    }
}
