use std::sync::Arc;

use super::{Action, Dialog, Encryption, EncryptionOutcome, Guard, Step, set_up};
use crate::app::uia::{AuthRound, Next, Reply, open_auth_page};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::EncryptionStep;
use crate::domain::encryption::IdentitySetup;
use crate::domain::uia::{AuthAnswer, AuthChallenge, Guarded};
use crate::ports::matrix::EncryptionPort;

impl Encryption {
    pub(super) fn abandon_reset(&mut self, port: Arc<dyn EncryptionPort>) {
        self.actions.spawn(async move {
            port.abandon_identity_reset().await;
        });
    }

    pub(in crate::app) fn start_reset(&mut self) {
        if !self.offers_reset() || self.pending.is_some() {
            tracing::debug!("the identity cannot be reset from here now");
            return;
        }
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        *dialog = Dialog::at(EncryptionStep::ConfirmingReset);
        self.publish();
    }

    pub(in crate::app) fn reset_identity(&mut self, port: Arc<dyn EncryptionPort>) {
        let confirming = self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.shows(&EncryptionStep::ConfirmingReset));
        if !confirming || !self.offers_reset() {
            tracing::debug!("dropping an identity reset nobody confirmed");
            return;
        }
        tracing::info!("resetting the account's cryptographic identity");
        self.act(Action::Reset, move || async move {
            match port.reset_identity().await {
                Ok(guarded) => EncryptionOutcome::Reset(Some(guarded)),
                Err(e) => {
                    tracing::warn!("could not reset the account's identity: {e}");
                    EncryptionOutcome::Reset(None)
                }
            }
        });
    }

    pub(in crate::app) fn awaits_auth(&self) -> bool {
        self.dialog
            .as_ref()
            .is_some_and(|dialog| matches!(dialog.step, Step::Authenticating { .. }))
    }

    pub(in crate::app) fn answer_auth(&mut self, port: Arc<dyn EncryptionPort>, reply: Reply) {
        if self.pending.is_some() {
            tracing::debug!("an answer is already on its way to the homeserver");
            return;
        }
        let Some(Dialog {
            step: Step::Authenticating { guard, round },
            error,
        }) = self.dialog.as_mut()
        else {
            return;
        };
        let Some(answer) = round.reply(reply) else {
            tracing::debug!("that answer does not fit the step the homeserver asks for");
            return;
        };
        *error = UserMessage::default();
        let guard = *guard;
        self.authenticate(port, guard, answer);
    }

    pub(in crate::app) fn reopen_auth_page(&mut self) {
        let Some(Dialog {
            step: Step::Authenticating { round, .. },
            ..
        }) = self.dialog.as_ref()
        else {
            return;
        };
        if let Some(page) = round.page().cloned() {
            open_auth_page(&mut self.actions, &self.browser, page);
        }
    }

    pub(in crate::app) fn cancel_auth(&mut self, port: Arc<dyn EncryptionPort>) {
        if self.pending.is_some() {
            tracing::debug!("an answer is on its way to the homeserver, not abandoning it");
            return;
        }
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        let Step::Authenticating { guard, .. } = dialog.step else {
            return;
        };
        tracing::debug!(
            ?guard,
            "abandoning the authentication the homeserver asked for"
        );
        *dialog = Dialog::at(EncryptionStep::Overview);
        if guard == Guard::Reset {
            self.abandon_reset(port);
        }
        self.publish();
    }

    pub(super) fn reset_settled(
        &mut self,
        port: Option<Arc<dyn EncryptionPort>>,
        guarded: Option<Guarded>,
    ) {
        match guarded {
            Some(Guarded::Done) => {
                tracing::info!("reset the account's cryptographic identity");
                self.setup = IdentitySetup::Settled;
                self.dialog = Some(Dialog::at(EncryptionStep::IdentityReset));
            }
            Some(Guarded::NeedsAuth(challenge)) => self.needs_auth(port, Guard::Reset, *challenge),
            None => {
                if let Some(dialog) = self.dialog.as_mut() {
                    *dialog = Dialog::at(EncryptionStep::ConfirmingReset);
                }
                self.report(UserMessageKind::IdentityResetFailed);
            }
        }
    }

    pub(super) fn needs_auth(
        &mut self,
        port: Option<Arc<dyn EncryptionPort>>,
        guard: Guard,
        challenge: AuthChallenge,
    ) {
        let Some(dialog) = self.dialog.as_ref() else {
            tracing::debug!("the dialog closed before the homeserver asked who you are");
            return;
        };
        let previous = match &dialog.step {
            Step::Authenticating { round, .. } => Some(round.as_ref()),
            Step::Shown(_) | Step::Exporting(_) | Step::Importing(_) => None,
        };
        match AuthRound::after(challenge, previous) {
            Next::Ask { round, open_page } => {
                if open_page && let Some(page) = round.page().cloned() {
                    open_auth_page(&mut self.actions, &self.browser, page);
                }
                self.set_step(Step::Authenticating {
                    guard,
                    round: Box::new(round),
                });
            }
            Next::Answer { round, answer } => {
                self.set_step(Step::Authenticating {
                    guard,
                    round: Box::new(round),
                });
                if let Some(port) = port {
                    self.authenticate(port, guard, answer);
                }
            }
            Next::Unsupported => {
                let back = match guard {
                    Guard::SetUp => EncryptionStep::Overview,
                    Guard::Reset => EncryptionStep::ConfirmingReset,
                };
                self.set_step(Step::Shown(back));
                self.report(UserMessageKind::AuthUnsupported);
            }
        }
    }

    fn authenticate(&mut self, port: Arc<dyn EncryptionPort>, guard: Guard, answer: AuthAnswer) {
        match guard {
            Guard::SetUp => self.act(Action::Authenticate, move || async move {
                EncryptionOutcome::IdentitySettled(set_up(port.as_ref(), Some(&answer)).await)
            }),
            Guard::Reset => self.act(Action::Authenticate, move || async move {
                match port.finish_identity_reset(&answer).await {
                    Ok(guarded) => EncryptionOutcome::Reset(Some(guarded)),
                    Err(e) => {
                        tracing::warn!("the identity reset failed: {e}");
                        EncryptionOutcome::Reset(None)
                    }
                }
            }),
        }
    }
}
