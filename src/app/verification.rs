use std::future::Future;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::event::AppEvent;
use super::task_group::TaskGroup;
use crate::commands::effects::{Effect, VerificationActivity, VerificationUpdate};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::domain::verification::VerificationEvent;
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

pub(super) struct VerificationController {
    output: Arc<dyn AppOutputPort>,
    events: mpsc::UnboundedSender<AppEvent>,
    flow: FlowState,
}

impl VerificationController {
    pub(super) fn new(
        output: Arc<dyn AppOutputPort>,
        events: mpsc::UnboundedSender<AppEvent>,
    ) -> Self {
        Self {
            output,
            events,
            flow: FlowState::Idle,
        }
    }

    pub(super) fn spawn_listener(
        &mut self,
        group: &mut TaskGroup,
        verification: Arc<dyn VerificationPort>,
    ) {
        self.flow = FlowState::Idle;
        let events = self.events.clone();
        let token = group.token();
        group.spawn(async move {
            let (verif_tx, mut verif_rx) = mpsc::unbounded_channel::<VerificationEvent>();
            let listen = verification.listen_for_verification(verif_tx);
            let forward = async {
                while let Some(event) = verif_rx.recv().await {
                    if events.send(AppEvent::VerificationFlow(event)).is_err() {
                        break;
                    }
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

    pub(super) async fn flow_advanced(&mut self, event: VerificationEvent) {
        self.flow = FlowState::of(&event);
        self.emit(VerificationUpdate::Flow(event)).await;
    }

    pub(super) async fn action_failed(&self, failure: UserMessageKind) {
        self.emit(VerificationUpdate::Failed(UserMessage::new(failure)))
            .await;
    }

    pub(super) async fn accept(
        &self,
        group: &mut TaskGroup,
        verification: Arc<dyn VerificationPort>,
    ) {
        self.act(
            group,
            verification,
            VerificationActivity::Accepting,
            UserMessageKind::VerificationAcceptFailed,
            |v| async move { v.accept_verification().await },
        )
        .await;
    }

    pub(super) async fn reject(
        &self,
        group: &mut TaskGroup,
        verification: Arc<dyn VerificationPort>,
    ) {
        self.act(
            group,
            verification,
            VerificationActivity::Declining,
            UserMessageKind::VerificationRejectFailed,
            |v| async move { v.reject_verification().await },
        )
        .await;
    }

    pub(super) async fn confirm(
        &self,
        group: &mut TaskGroup,
        verification: Arc<dyn VerificationPort>,
    ) {
        self.act(
            group,
            verification,
            VerificationActivity::Confirming,
            UserMessageKind::VerificationConfirmFailed,
            |v| async move { v.confirm_verification().await },
        )
        .await;
    }

    pub(super) async fn dismiss(
        &mut self,
        group: &mut TaskGroup,
        verification: Option<Arc<dyn VerificationPort>>,
    ) {
        if self.flow == FlowState::Active
            && let Some(verification) = verification
        {
            tracing::info!("dismissing a live verification; cancelling it first");
            self.reject(group, verification).await;
            return;
        }
        self.flow = FlowState::Idle;
        self.emit(VerificationUpdate::Dismissed).await;
    }

    pub(super) fn reset(&mut self) {
        self.flow = FlowState::Idle;
    }

    async fn act<F, Fut>(
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
        self.emit(VerificationUpdate::Busy(activity)).await;
        let events = self.events.clone();
        group.spawn(async move {
            if let Err(e) = action(verification).await {
                tracing::warn!("verification action failed: {e}");
                if events
                    .send(AppEvent::VerificationActionFailed(failure))
                    .is_err()
                {
                    tracing::debug!("the app event loop is gone; dropping a verification failure");
                }
            }
        });
    }

    async fn emit(&self, update: VerificationUpdate) {
        self.output.emit(Effect::Verification(update)).await;
    }
}
