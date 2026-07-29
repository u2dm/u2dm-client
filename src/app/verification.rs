use std::future::Future;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};

use tokio::sync::mpsc;

use super::task_group::TaskGroup;
use crate::commands::effects::{Effect, VerificationActivity, VerificationUpdate};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::domain::models::VerificationEvent;
use crate::error::Result;
use crate::ports::matrix::VerificationPort;
use crate::ports::output::AppOutputPort;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum FlowState {
    #[default]
    Idle,
    Active,
    Terminal,
}

impl FlowState {
    fn of(event: &VerificationEvent) -> Self {
        match event {
            VerificationEvent::Requested { .. }
            | VerificationEvent::Emojis(_)
            | VerificationEvent::Confirming => Self::Active,
            VerificationEvent::Done | VerificationEvent::Cancelled(_) => Self::Terminal,
        }
    }
}

#[derive(Clone)]
pub(super) struct VerificationController {
    output: Arc<dyn AppOutputPort>,
    flow: Arc<StdMutex<FlowState>>,
}

impl VerificationController {
    pub(super) fn new(output: Arc<dyn AppOutputPort>) -> Self {
        Self {
            output,
            flow: Arc::new(StdMutex::new(FlowState::Idle)),
        }
    }

    fn flow(&self) -> FlowState {
        *self.flow.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set_flow(flow: &StdMutex<FlowState>, state: FlowState) {
        *flow.lock().unwrap_or_else(PoisonError::into_inner) = state;
    }

    fn take_dismissable(flow: &StdMutex<FlowState>) -> bool {
        let mut state = flow.lock().unwrap_or_else(PoisonError::into_inner);
        if *state == FlowState::Active {
            return false;
        }
        *state = FlowState::Idle;
        true
    }

    pub(super) fn spawn_forwarder(
        &self,
        group: &mut TaskGroup,
        verification: Arc<dyn VerificationPort>,
    ) {
        let output = Arc::clone(&self.output);
        let flow = Arc::clone(&self.flow);
        Self::set_flow(&flow, FlowState::Idle);
        let token = group.token();
        group.spawn(async move {
            let (verif_tx, mut verif_rx) = mpsc::unbounded_channel::<VerificationEvent>();
            let listen = verification.listen_for_verification(verif_tx);
            let forward = async {
                while let Some(event) = verif_rx.recv().await {
                    Self::set_flow(&flow, FlowState::of(&event));
                    output
                        .emit(Effect::Verification(VerificationUpdate::Flow(event)))
                        .await;
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

    fn spawn_action<F, Fut>(
        &self,
        group: &mut TaskGroup,
        verification: Arc<dyn VerificationPort>,
        activity: VerificationActivity,
        failure: UserMessageKind,
        action: F,
    ) where
        F: FnOnce(Arc<dyn VerificationPort>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send,
    {
        let output = Arc::clone(&self.output);
        group.spawn(async move {
            output
                .emit(Effect::Verification(VerificationUpdate::Busy(activity)))
                .await;
            if let Err(e) = action(verification).await {
                tracing::warn!("verification action failed: {e}");
                output
                    .emit(Effect::Verification(VerificationUpdate::Failed(
                        UserMessage::new(failure),
                    )))
                    .await;
            }
        });
    }

    pub(super) fn spawn_accept(
        &self,
        group: &mut TaskGroup,
        verification: Arc<dyn VerificationPort>,
    ) {
        self.spawn_action(
            group,
            verification,
            VerificationActivity::Accepting,
            UserMessageKind::VerificationAcceptFailed,
            |v| async move { v.accept_verification().await },
        );
    }

    pub(super) fn spawn_reject(
        &self,
        group: &mut TaskGroup,
        verification: Arc<dyn VerificationPort>,
    ) {
        self.spawn_action(
            group,
            verification,
            VerificationActivity::Declining,
            UserMessageKind::VerificationRejectFailed,
            |v| async move { v.reject_verification().await },
        );
    }

    pub(super) fn spawn_confirm(
        &self,
        group: &mut TaskGroup,
        verification: Arc<dyn VerificationPort>,
    ) {
        self.spawn_action(
            group,
            verification,
            VerificationActivity::Confirming,
            UserMessageKind::VerificationConfirmFailed,
            |v| async move { v.confirm_verification().await },
        );
    }

    pub(super) fn spawn_dismiss(
        &self,
        group: &mut TaskGroup,
        verification: Option<Arc<dyn VerificationPort>>,
    ) {
        if self.flow() == FlowState::Active
            && let Some(verification) = verification
        {
            tracing::info!("dismissing a live verification; cancelling it first");
            self.spawn_reject(group, verification);
            return;
        }
        let output = Arc::clone(&self.output);
        let flow = Arc::clone(&self.flow);
        group.spawn(async move {
            if !Self::take_dismissable(&flow) {
                tracing::debug!("a verification started before the dismissal ran; keeping it up");
                return;
            }
            output
                .emit(Effect::Verification(VerificationUpdate::Dismissed))
                .await;
        });
    }
}
