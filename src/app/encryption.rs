use std::sync::Arc;

use tokio::sync::mpsc;

use super::event::{AppEvent, EncryptionEvent};
use super::input::EventSender;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{
    EncryptionActivity, EncryptionDialog, EncryptionStep, EncryptionView, Toast,
};
use crate::domain::encryption::{
    AccountEncryption, IdentityOutcome, IdentitySetup, Recovery, RecoveryAttempt, RecoveryKey,
    RecoverySetup, Standing,
};
use crate::domain::identity::SessionAlert;
use crate::ports::matrix::{EncryptionPort, VerificationPort};
use crate::ports::output::AppOutputPort;

const STATE_CHANNEL_CAP: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Request,
    Unlock,
    SetUp,
    Change,
    Retry,
}

impl Action {
    fn activity(self) -> EncryptionActivity {
        match self {
            Self::Request => EncryptionActivity::Requesting,
            Self::Unlock => EncryptionActivity::Unlocking,
            Self::SetUp => EncryptionActivity::SettingUpRecovery,
            Self::Change => EncryptionActivity::ChangingKey,
            Self::Retry => EncryptionActivity::RetryingSetup,
        }
    }
}

pub(super) enum EncryptionOutcome {
    Requested,
    Tried(RecoveryAttempt),
    SetUp(RecoverySetup),
    Changed(RecoveryKey),
    IdentitySettled(Option<IdentityOutcome>),
    Failed(UserMessageKind),
}

struct Pending {
    request: u64,
    action: Action,
}

struct Dialog {
    step: EncryptionStep,
    error: UserMessage,
}

impl Dialog {
    fn at(step: EncryptionStep) -> Self {
        Self {
            step,
            error: UserMessage::default(),
        }
    }
}

pub(super) struct Encryption {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    actions: TaskGroup,
    requests: u64,
    first_request_of_session: u64,
    state: AccountEncryption,
    setup: IdentitySetup,
    dialog: Option<Dialog>,
    pending: Option<Pending>,
}

impl Encryption {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            actions: TaskGroup::new("encryption-actions"),
            requests: 0,
            first_request_of_session: 1,
            state: AccountEncryption::default(),
            setup: IdentitySetup::Running,
            dialog: None,
            pending: None,
        }
    }

    pub(super) fn begin_session(
        &mut self,
        group: &mut TaskGroup,
        port: &Arc<dyn EncryptionPort>,
        session: u64,
    ) {
        self.state = AccountEncryption::default();
        self.setup = IdentitySetup::Running;
        self.publish();

        let follow_port = Arc::clone(port);
        let events = self.events.clone();
        let token = group.token();
        group.spawn(async move {
            let (state_tx, state_rx) = mpsc::channel(STATE_CHANNEL_CAP);
            let follow = async {
                tokio::join!(
                    follow(follow_port.as_ref(), state_tx),
                    forward_states(&events, session, state_rx),
                )
            };
            token.run_until_cancelled(follow).await;
        });

        let setup_port = Arc::clone(port);
        let events = self.events.clone();
        let token = group.token();
        group.spawn(async move {
            let Some(outcome) = token.run_until_cancelled(set_up(setup_port.as_ref())).await else {
                return;
            };
            drop(
                events.send(AppEvent::Encryption(EncryptionEvent::IdentitySettled {
                    session,
                    outcome,
                })),
            );
        });
    }

    pub(super) fn state_changed(&mut self, state: AccountEncryption) {
        self.state = state;
        self.fall_back_from_unoffered_step();
        self.publish();
    }

    pub(super) fn identity_settled(&mut self, outcome: Option<IdentityOutcome>) {
        self.setup = match outcome {
            Some(IdentityOutcome::Ready) => IdentitySetup::Settled,
            Some(IdentityOutcome::NeedsAuthentication) => IdentitySetup::NeedsAuthentication,
            None => IdentitySetup::Failed,
        };
        self.publish();
    }

    pub(super) fn alert(&self) -> Option<SessionAlert> {
        self.standing().alert()
    }

    pub(super) fn open(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        tracing::debug!("opening the encryption dialog");
        self.dialog = Some(Dialog::at(EncryptionStep::Overview));
        self.publish();
    }

    pub(super) fn close(&mut self) {
        if self.dialog.take().is_none() {
            return;
        }
        tracing::debug!("closing the encryption dialog");
        self.publish();
    }

    pub(super) fn show_overview(&mut self) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        if self.pending.is_some() && dialog.step == EncryptionStep::EnteringKey {
            return;
        }
        *dialog = Dialog::at(EncryptionStep::Overview);
        self.publish();
    }

    pub(super) fn enter_key(&mut self) {
        if !self.state.recovery_unlocks() || self.pending.is_some() {
            tracing::debug!("not offering the recovery key entry now");
            return;
        }
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        *dialog = Dialog::at(EncryptionStep::EnteringKey);
        self.publish();
    }

    pub(super) fn use_key(&mut self, port: Arc<dyn EncryptionPort>, key: RecoveryKey) {
        let entering = self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.step == EncryptionStep::EnteringKey);
        if !entering {
            tracing::debug!("dropping a recovery key typed into a closed entry");
            return;
        }
        self.act(Action::Unlock, move || async move {
            match port.recover(&key).await {
                Ok(attempt) => EncryptionOutcome::Tried(attempt),
                Err(e) => {
                    tracing::warn!("could not use the recovery key: {e}");
                    EncryptionOutcome::Failed(UserMessageKind::RecoveryKeyFailed)
                }
            }
        });
    }

    pub(super) fn verify_with_other_session(&mut self, port: Arc<dyn VerificationPort>) {
        if self.standing() != Standing::Unverified || !self.state.verified_sessions {
            tracing::debug!("no other session can verify this one now");
            return;
        }
        self.act(Action::Request, move || async move {
            match port.verify_this_session().await {
                Ok(()) => EncryptionOutcome::Requested,
                Err(e) => {
                    tracing::warn!("could not ask the other sessions to verify this one: {e}");
                    EncryptionOutcome::Failed(UserMessageKind::VerificationRequestFailed)
                }
            }
        });
    }

    pub(super) fn set_up_recovery(&mut self, port: Arc<dyn EncryptionPort>) {
        if self.standing() != Standing::Verified || self.state.recovery != Recovery::Off {
            tracing::debug!("recovery cannot be set up from here now");
            return;
        }
        self.act(Action::SetUp, move || async move {
            match port.enable_recovery().await {
                Ok(setup) => EncryptionOutcome::SetUp(setup),
                Err(e) => {
                    tracing::warn!("could not set up recovery: {e}");
                    EncryptionOutcome::Failed(UserMessageKind::RecoverySetupFailed)
                }
            }
        });
    }

    pub(super) fn change_key(&mut self, port: Arc<dyn EncryptionPort>) {
        if self.standing() != Standing::Verified || self.state.recovery != Recovery::On {
            tracing::debug!("the recovery key cannot be changed from here now");
            return;
        }
        self.act(Action::Change, move || async move {
            match port.change_recovery_key().await {
                Ok(key) => EncryptionOutcome::Changed(key),
                Err(e) => {
                    tracing::warn!("could not change the recovery key: {e}");
                    EncryptionOutcome::Failed(UserMessageKind::RecoveryKeyChangeFailed)
                }
            }
        });
    }

    pub(super) fn retry_setup(&mut self, port: Arc<dyn EncryptionPort>) {
        if !matches!(
            self.standing(),
            Standing::SetupRefused | Standing::SetupFailed
        ) || self.pending.is_some()
        {
            tracing::debug!("the account's identity needs no retry now");
            return;
        }
        self.setup = IdentitySetup::Running;
        self.act(Action::Retry, move || async move {
            EncryptionOutcome::IdentitySettled(set_up(port.as_ref()).await)
        });
    }

    pub(super) fn settled(&mut self, request: u64, outcome: EncryptionOutcome) {
        if request < self.first_request_of_session {
            tracing::debug!(
                request,
                "dropping an encryption action of a session that ended"
            );
            return;
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.request == request)
        {
            self.pending = None;
        }
        match outcome {
            EncryptionOutcome::Requested => self.close(),
            EncryptionOutcome::Tried(attempt) => self.tried(attempt),
            EncryptionOutcome::SetUp(RecoverySetup::Created(key))
            | EncryptionOutcome::Changed(key) => self.show_key(key),
            EncryptionOutcome::SetUp(RecoverySetup::BackupInTheWay) => {
                self.report(UserMessageKind::RecoveryBackupInTheWay);
            }
            EncryptionOutcome::IdentitySettled(outcome) => self.identity_settled(outcome),
            EncryptionOutcome::Failed(kind) => self.report(kind),
        }
        self.publish();
    }

    pub(super) async fn restart(&mut self) {
        self.actions.restart().await;
        self.first_request_of_session = self.requests.wrapping_add(1);
        self.state = AccountEncryption::default();
        self.setup = IdentitySetup::Running;
        self.dialog = None;
        self.pending = None;
    }

    pub(super) async fn shutdown(&mut self) {
        self.actions.shutdown().await;
    }

    fn standing(&self) -> Standing {
        Standing::of(self.state.trust, self.setup)
    }

    fn tried(&mut self, attempt: RecoveryAttempt) {
        let step = match attempt {
            RecoveryAttempt::Unlocked => EncryptionStep::Unlocked,
            RecoveryAttempt::UnlockedWithoutBackup => EncryptionStep::UnlockedWithoutBackup,
            RecoveryAttempt::WrongKey => {
                self.report(UserMessageKind::RecoveryKeyWrong);
                return;
            }
        };
        if let Some(dialog) = self.dialog.as_mut() {
            *dialog = Dialog::at(step);
        }
    }

    fn show_key(&mut self, key: RecoveryKey) {
        if self.dialog.is_none() {
            tracing::info!("reopening the encryption dialog to show the new recovery key");
        }
        self.dialog = Some(Dialog::at(EncryptionStep::ShowingKey(key)));
    }

    fn report(&mut self, kind: UserMessageKind) {
        match self.dialog.as_mut() {
            Some(dialog) => dialog.error = UserMessage::new(kind),
            None => show_toast(self.output.as_ref(), Toast::Error(UserMessage::new(kind))),
        }
    }

    fn fall_back_from_unoffered_step(&mut self) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        if dialog.step == EncryptionStep::EnteringKey
            && self.pending.is_none()
            && !self.state.recovery_unlocks()
        {
            *dialog = Dialog::at(EncryptionStep::Overview);
        }
    }

    fn act<F, Fut>(&mut self, action: Action, work: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = EncryptionOutcome> + Send,
    {
        if let Some(pending) = &self.pending {
            tracing::debug!(running = ?pending.action, ?action, "an encryption action is already running");
            return;
        }
        self.requests = self.requests.wrapping_add(1);
        let request = self.requests;
        self.pending = Some(Pending { request, action });
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.error = UserMessage::default();
        }
        self.publish();

        let events = self.events.clone();
        self.actions.spawn(async move {
            let outcome = work().await;
            drop(
                events.send(AppEvent::Encryption(EncryptionEvent::ActionSettled {
                    request,
                    outcome,
                })),
            );
        });
    }

    fn publish(&self) {
        let activity = self
            .pending
            .as_ref()
            .map_or(EncryptionActivity::None, |pending| {
                pending.action.activity()
            });
        let view = EncryptionView {
            standing: self.standing(),
            verified_sessions: self.state.verified_sessions,
            recovery: self.state.recovery,
            key_storage: self.state.key_storage,
            dialog: self.dialog.as_ref().map(|dialog| EncryptionDialog {
                step: dialog.step.clone(),
                activity,
                error: dialog.error.clone(),
            }),
        };
        self.output
            .publish(Box::new(move |state| state.encryption = view));
    }
}

async fn set_up(port: &dyn EncryptionPort) -> Option<IdentityOutcome> {
    match port.set_up_identity().await {
        Ok(outcome) => Some(outcome),
        Err(e) => {
            tracing::warn!("could not set up the account's cross-signing identity: {e}");
            None
        }
    }
}

async fn follow(port: &dyn EncryptionPort, state_tx: mpsc::Sender<AccountEncryption>) {
    if let Err(e) = port.follow(state_tx).await {
        tracing::warn!("failed to follow this session's encryption: {e}");
    }
}

async fn forward_states(
    events: &EventSender,
    session: u64,
    mut state_rx: mpsc::Receiver<AccountEncryption>,
) {
    while let Some(state) = state_rx.recv().await {
        let event = AppEvent::Encryption(EncryptionEvent::Changed { session, state });
        if events.send(event).is_err() {
            return;
        }
    }
}
