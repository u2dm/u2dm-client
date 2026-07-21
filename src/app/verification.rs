use std::sync::Arc;

use tokio::sync::mpsc;

use super::task_group::TaskGroup;
use crate::commands::Effect;
use crate::domain::models::VerificationEvent;
use crate::ports::matrix::VerificationPort;
use crate::ports::output::AppOutputPort;

#[derive(Clone)]
pub(super) struct VerificationController {
    output: Arc<dyn AppOutputPort>,
}

impl VerificationController {
    pub(super) fn new(output: Arc<dyn AppOutputPort>) -> Self {
        Self { output }
    }

    pub(super) fn spawn_forwarder(
        &self,
        group: &mut TaskGroup,
        verification: Arc<dyn VerificationPort>,
    ) {
        let output = Arc::clone(&self.output);
        let token = group.token();
        group.spawn(async move {
            let (verif_tx, mut verif_rx) = mpsc::unbounded_channel::<VerificationEvent>();
            let listen = verification.listen_for_verification(verif_tx);
            let forward = async {
                while let Some(event) = verif_rx.recv().await {
                    output.emit(Effect::Verification(event)).await;
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

    pub(super) fn spawn_accept(
        &self,
        group: &mut TaskGroup,
        verification: Arc<dyn VerificationPort>,
    ) {
        let output = Arc::clone(&self.output);
        group.spawn(async move {
            if let Err(e) = verification.accept_verification().await {
                tracing::warn!("verification accept failed: {e}");
                output
                    .emit(Effect::Toast(format!("Verification accept failed: {e}")))
                    .await;
            }
        });
    }

    pub(super) fn spawn_reject(
        &self,
        group: &mut TaskGroup,
        verification: Arc<dyn VerificationPort>,
    ) {
        let output = Arc::clone(&self.output);
        group.spawn(async move {
            if let Err(e) = verification.reject_verification().await {
                tracing::warn!("verification reject failed: {e}");
                output
                    .emit(Effect::Toast(format!("Verification reject failed: {e}")))
                    .await;
            }
        });
    }

    pub(super) fn spawn_confirm(
        &self,
        group: &mut TaskGroup,
        verification: Arc<dyn VerificationPort>,
    ) {
        let output = Arc::clone(&self.output);
        group.spawn(async move {
            if let Err(e) = verification.confirm_verification().await {
                tracing::warn!("verification confirm failed: {e}");
                output
                    .emit(Effect::Toast(format!("Verification confirm failed: {e}")))
                    .await;
            }
        });
    }
}
