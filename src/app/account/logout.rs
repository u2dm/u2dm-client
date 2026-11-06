use std::sync::Arc;

use super::{Account, LogoutDecision};
use crate::app::event::{AccountEvent, AppEvent};
use crate::commands::view::LogoutView;
use crate::domain::sessions::LogoutRisks;
use crate::ports::matrix::AccountPort;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Logout {
    Idle,
    Checking(u64),
    Warning(LogoutRisks),
}

impl Account {
    pub(in crate::app) fn check_logout(&mut self, port: Arc<dyn AccountPort>) {
        if self.logout != Logout::Idle {
            tracing::debug!("the logout is already being checked or waits for an answer");
            return;
        }
        let request = self.requests.issue();
        self.logout = Logout::Checking(request);
        let events = self.events.clone();
        self.actions.spawn(async move {
            let risks = port.logout_risks().await;
            drop(events.send(AppEvent::Account(AccountEvent::LogoutChecked {
                request,
                risks,
            })));
        });
    }

    pub(in crate::app) fn logout_checked(
        &mut self,
        request: u64,
        risks: LogoutRisks,
    ) -> LogoutDecision {
        if self.logout != Logout::Checking(request) {
            return LogoutDecision::Wait;
        }
        self.logout = Logout::Idle;
        tracing::info!(
            keys_at_risk = risks.keys_at_risk,
            unsent = risks.unsent,
            "checked what logging out would lose"
        );
        if !risks.any() {
            return LogoutDecision::Proceed;
        }
        self.logout = Logout::Warning(risks);
        self.publish_logout();
        LogoutDecision::Wait
    }

    pub(in crate::app) fn confirm_logout(&mut self) -> bool {
        if !self.drop_logout_warning() {
            return false;
        }
        self.publish_logout();
        true
    }

    pub(in crate::app) fn cancel_logout(&mut self) {
        if self.drop_logout_warning() {
            self.publish_logout();
        }
    }

    fn drop_logout_warning(&mut self) -> bool {
        let warned = matches!(self.logout, Logout::Warning(_));
        if warned {
            self.logout = Logout::Idle;
        }
        warned
    }

    fn publish_logout(&self) {
        let warning = match self.logout {
            Logout::Warning(risks) => Some(risks),
            Logout::Idle | Logout::Checking(_) => None,
        };
        let view = LogoutView { warning };
        self.output
            .publish(Box::new(move |state| state.logout = view));
    }
}
