use std::sync::Arc;

use super::operation::Operation;
use super::{Account, Dialog, Step};
use crate::app::uia::{AuthRound, Next, Reply, open_auth_page};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::domain::capabilities::AccountPage;
use crate::domain::uia::AuthChallenge;
use crate::ports::matrix::AccountPort;

impl Account {
    pub(in crate::app) fn awaits_auth(&self) -> bool {
        self.dialog
            .as_ref()
            .is_some_and(|dialog| matches!(dialog.step, Step::Authenticating { .. }))
    }

    pub(in crate::app) fn answer_auth(&mut self, port: Arc<dyn AccountPort>, reply: Reply) {
        if self.pending.is_some() {
            tracing::debug!("an answer is already on its way to the homeserver");
            return;
        }
        let Some(Dialog {
            step: Step::Authenticating { operation, round },
            ..
        }) = self.dialog.as_mut()
        else {
            return;
        };
        let Some(answer) = round.reply(reply) else {
            tracing::debug!("that answer does not fit the step the homeserver asks for");
            return;
        };
        let operation = operation.clone();
        self.start(port, operation, Some(answer));
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

    pub(in crate::app) fn cancel_auth(&mut self) {
        if self.pending.is_some() {
            tracing::debug!("an answer is on its way to the homeserver, not abandoning it");
            return;
        }
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        let Step::Authenticating { operation, .. } = &dialog.step else {
            return;
        };
        tracing::debug!("abandoning the authentication the homeserver asked for");
        dialog.step = operation.origin();
        dialog.error = UserMessage::default();
        self.publish();
    }

    pub(super) fn needs_auth(
        &mut self,
        port: Option<Arc<dyn AccountPort>>,
        operation: Operation,
        challenge: AuthChallenge,
    ) {
        let Some(dialog) = self.dialog.as_ref() else {
            tracing::debug!("the dialog closed before the homeserver asked who you are");
            return;
        };
        let previous = match &dialog.step {
            Step::Authenticating { round, .. } => Some(round.as_ref()),
            Step::Overview | Step::ChangingPassword | Step::ConfirmingDeactivation => None,
        };
        match AuthRound::after(challenge, previous) {
            Next::Ask { round, open_page } => {
                if open_page && let Some(page) = round.page() {
                    open_auth_page(&mut self.actions, &self.browser, page.clone());
                }
                self.set_step(Step::Authenticating {
                    operation,
                    round: Box::new(round),
                });
            }
            Next::Answer { round, answer } => {
                self.set_step(Step::Authenticating {
                    operation: operation.clone(),
                    round: Box::new(round),
                });
                if let Some(port) = port {
                    self.start(port, operation, Some(answer));
                }
            }
            Next::Unsupported => {
                self.set_step(operation.origin());
                self.report(UserMessage::new(UserMessageKind::AuthUnsupported));
            }
        }
    }

    pub(super) fn open_page(&mut self, port: Arc<dyn AccountPort>, page: AccountPage) {
        tracing::info!(?page, "opening the homeserver's account page");
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.error = UserMessage::default();
            dialog.notice = UserMessage::new(UserMessageKind::AccountPageOpened);
        }
        self.publish();
        let browser = Arc::clone(&self.browser);
        self.actions.spawn(async move {
            let Some(url) = port.page(&page).await else {
                tracing::warn!(?page, "the homeserver offers no page for that");
                return;
            };
            if let Err(e) = browser.open_url(&url).await {
                tracing::warn!("the account page could not be opened: {e}");
            }
        });
    }
}
