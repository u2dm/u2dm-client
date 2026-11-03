use std::sync::Arc;

use tokio::sync::mpsc;

use super::event::AppEvent;
use super::input::EventSender;
use super::task_group::TaskGroup;
use crate::commands::effects::{Effect, VerificationActivity, VerificationUpdate};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::domain::verification::VerificationEvent;
use crate::error::Result;
use crate::ports::matrix::VerificationPort;
use crate::ports::output::AppOutputPort;

#[derive(Clone, Copy)]
pub(super) enum VerificationAction {
    Accept,
    Reject,
    Confirm,
    Cancel,
}

impl VerificationAction {
    fn activity(self) -> VerificationActivity {
        match self {
            Self::Accept => VerificationActivity::Accepting,
            Self::Reject | Self::Cancel => VerificationActivity::Declining,
            Self::Confirm => VerificationActivity::Confirming,
        }
    }

    fn failure(self) -> UserMessageKind {
        match self {
            Self::Accept => UserMessageKind::VerificationAcceptFailed,
            Self::Reject | Self::Cancel => UserMessageKind::VerificationRejectFailed,
            Self::Confirm => UserMessageKind::VerificationConfirmFailed,
        }
    }

    async fn run(self, port: &dyn VerificationPort) -> Result<()> {
        match self {
            Self::Accept => port.accept_verification().await,
            Self::Reject => port.reject_verification().await,
            Self::Confirm => port.confirm_verification().await,
            Self::Cancel => port.cancel_verification().await,
        }
    }
}

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
            | VerificationEvent::RequestSent { .. }
            | VerificationEvent::Emojis(_)
            | VerificationEvent::Confirming => Self::Active,
            VerificationEvent::Done | VerificationEvent::Cancelled(_) => Self::Terminal,
        }
    }
}

pub(super) struct VerificationController {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    session: u64,
    flow: FlowState,
}

impl VerificationController {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            session: 0,
            flow: FlowState::Idle,
        }
    }

    pub(super) fn spawn_listener(
        &mut self,
        group: &mut TaskGroup,
        verification: Arc<dyn VerificationPort>,
        session: u64,
    ) {
        self.session = session;
        self.flow = FlowState::Idle;
        let events = self.events.clone();
        let token = group.token();
        group.spawn(async move {
            let (verif_tx, mut verif_rx) = mpsc::unbounded_channel::<VerificationEvent>();
            let listen = verification.listen_for_verification(verif_tx);
            let forward = async {
                while let Some(event) = verif_rx.recv().await {
                    if events
                        .send(AppEvent::VerificationFlow { session, event })
                        .is_err()
                    {
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

    pub(super) async fn dismiss(
        &mut self,
        group: &mut TaskGroup,
        verification: Option<Arc<dyn VerificationPort>>,
    ) {
        if self.flow == FlowState::Active
            && let Some(verification) = verification
        {
            tracing::info!("dismissing a live verification; cancelling it first");
            self.act(group, verification, VerificationAction::Cancel)
                .await;
            return;
        }
        self.flow = FlowState::Idle;
        self.emit(VerificationUpdate::Dismissed).await;
    }

    pub(super) fn reset(&mut self) {
        self.flow = FlowState::Idle;
    }

    pub(super) async fn act(
        &self,
        group: &mut TaskGroup,
        verification: Arc<dyn VerificationPort>,
        action: VerificationAction,
    ) {
        self.emit(VerificationUpdate::Busy(action.activity())).await;
        let events = self.events.clone();
        let session = self.session;
        let failure = action.failure();
        group.spawn(async move {
            if let Err(e) = action.run(verification.as_ref()).await {
                tracing::warn!("verification action failed: {e}");
                if events
                    .send(AppEvent::VerificationActionFailed { session, failure })
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
