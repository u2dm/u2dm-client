use std::sync::Arc;

use tokio::sync::mpsc;

use super::event::{AccountEvent, AppEvent};
use super::input::EventSender;
use super::show_toast;
use super::task_group::TaskGroup;
use super::uia::{AuthRound, Next, Reply};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{
    AccountActivity, AccountAuth, AccountDialog, AccountStep, LogoutView, Toast,
};
use crate::domain::capabilities::{AccountApi, AccountFacts, AccountPage};
use crate::domain::link::LauncherSafeUrl;
use crate::domain::sessions::{LogoutRisks, OwnSessions};
use crate::domain::uia::{AccountPassword, AuthAnswer, AuthChallenge, Guarded, is_complex_enough};
use crate::error::{AppError, Result as AppResult};
use crate::ports::browser::BrowserPort;
use crate::ports::matrix::AccountPort;
use crate::ports::output::AppOutputPort;

const SESSIONS_CHANNEL_CAP: usize = 4;

#[derive(Clone)]
enum Operation {
    SignOut {
        device_id: String,
        name: String,
    },
    ChangePassword {
        password: AccountPassword,
        sign_out_others: bool,
    },
    Deactivate {
        erase: bool,
    },
}

impl Operation {
    fn activity(&self) -> AccountActivity {
        match self {
            Self::SignOut { .. } => AccountActivity::SigningOut,
            Self::ChangePassword { .. } => AccountActivity::ChangingPassword,
            Self::Deactivate { .. } => AccountActivity::Deactivating,
        }
    }

    fn origin(&self) -> Step {
        match self {
            Self::SignOut { .. } => Step::Overview,
            Self::ChangePassword { .. } => Step::ChangingPassword,
            Self::Deactivate { .. } => Step::ConfirmingDeactivation,
        }
    }

    fn origin_step(&self) -> AccountStep {
        match self {
            Self::SignOut { .. } => AccountStep::Overview,
            Self::ChangePassword { .. } => AccountStep::ChangingPassword,
            Self::Deactivate { .. } => AccountStep::ConfirmingDeactivation,
        }
    }

    fn subject(&self) -> String {
        match self {
            Self::SignOut { name, .. } => name.clone(),
            Self::ChangePassword { .. } | Self::Deactivate { .. } => String::new(),
        }
    }

    fn device(&self) -> Option<&str> {
        match self {
            Self::SignOut { device_id, .. } => Some(device_id),
            Self::ChangePassword { .. } | Self::Deactivate { .. } => None,
        }
    }

    fn failure(&self, error: &AppError) -> UserMessage {
        match (self, error) {
            (Self::ChangePassword { .. }, AppError::WeakPassword(reason)) => {
                UserMessage::about(UserMessageKind::NewPasswordRefused, reason)
            }
            (Self::SignOut { name, .. }, _) => {
                UserMessage::about(UserMessageKind::SessionSignOutFailed, name)
            }
            (Self::ChangePassword { .. }, _) => {
                UserMessage::new(UserMessageKind::PasswordChangeFailed)
            }
            (Self::Deactivate { .. }, _) => UserMessage::new(UserMessageKind::DeactivationFailed),
        }
    }

    async fn run(&self, port: &dyn AccountPort, auth: Option<&AuthAnswer>) -> AppResult<Guarded> {
        match self {
            Self::SignOut { device_id, .. } => port.sign_out_session(device_id, auth).await,
            Self::ChangePassword {
                password,
                sign_out_others,
            } => port.change_password(password, *sign_out_others, auth).await,
            Self::Deactivate { erase } => port.deactivate(*erase, auth).await,
        }
    }
}

enum Step {
    Overview,
    ChangingPassword,
    ConfirmingDeactivation,
    Authenticating {
        operation: Operation,
        round: Box<AuthRound>,
    },
}

struct Dialog {
    generation: u64,
    step: Step,
    facts: Option<AccountFacts>,
    sessions: Option<OwnSessions>,
    error: UserMessage,
    notice: UserMessage,
}

enum Work {
    Rename { device_id: String },
    Guarded(Operation),
}

struct Pending {
    request: u64,
    work: Work,
}

pub(super) enum AccountOutcome {
    Renamed(Result<(), UserMessage>),
    Guarded(Result<Guarded, UserMessage>),
}

pub(super) enum Settled {
    Nothing,
    Deactivated,
}

pub(super) enum LogoutDecision {
    Wait,
    Proceed,
}

pub(super) struct Account {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    browser: Arc<dyn BrowserPort>,
    tasks: TaskGroup,
    actions: TaskGroup,
    generations: u64,
    requests: u64,
    first_request_of_session: u64,
    dialog: Option<Dialog>,
    pending: Option<Pending>,
    logout_check: Option<u64>,
    logout_warning: Option<LogoutRisks>,
}

impl Account {
    pub(super) fn new(
        output: Arc<dyn AppOutputPort>,
        events: EventSender,
        browser: Arc<dyn BrowserPort>,
    ) -> Self {
        Self {
            output,
            events,
            browser,
            tasks: TaskGroup::new("account-follow"),
            actions: TaskGroup::new("account-actions"),
            generations: 0,
            requests: 0,
            first_request_of_session: 1,
            dialog: None,
            pending: None,
            logout_check: None,
            logout_warning: None,
        }
    }

    pub(super) fn open(&mut self, port: &Arc<dyn AccountPort>) {
        if self.dialog.is_some() {
            return;
        }
        tracing::debug!("opening the account dialog");
        self.generations = self.generations.wrapping_add(1);
        let generation = self.generations;
        self.dialog = Some(Dialog {
            generation,
            step: Step::Overview,
            facts: None,
            sessions: None,
            error: UserMessage::default(),
            notice: UserMessage::default(),
        });
        self.publish();
        self.follow(port, generation);
    }

    pub(super) fn close(&mut self) {
        if self.dialog.take().is_none() {
            return;
        }
        tracing::debug!("closing the account dialog");
        self.tasks.cancel_and_detach();
        self.publish();
    }

    pub(super) fn show_overview(&mut self) {
        let busy = self.pending.is_some();
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        if busy && matches!(dialog.step, Step::Authenticating { .. }) {
            return;
        }
        dialog.step = Step::Overview;
        dialog.error = UserMessage::default();
        self.publish();
    }

    pub(super) fn facts_read(&mut self, generation: u64, facts: AccountFacts) {
        let Some(dialog) = self.current_dialog(generation) else {
            return;
        };
        dialog.facts = Some(facts);
        self.publish();
    }

    pub(super) fn sessions_changed(&mut self, generation: u64, sessions: OwnSessions) {
        let Some(dialog) = self.current_dialog(generation) else {
            return;
        };
        dialog.sessions = Some(sessions);
        self.publish();
    }

    pub(super) fn rename_session(
        &mut self,
        port: Arc<dyn AccountPort>,
        device_id: String,
        name: &str,
    ) {
        let name = name.trim().to_owned();
        let Some(dialog) = self.dialog.as_ref() else {
            return;
        };
        if name.is_empty() || self.api().is_oauth() || !self.knows_session(&device_id) {
            tracing::debug!(%device_id, "not renaming this session from here");
            return;
        }
        if !matches!(dialog.step, Step::Overview) {
            return;
        }
        let target = device_id.clone();
        self.act(Work::Rename { device_id }, move || async move {
            match port.rename_session(&target, &name).await {
                Ok(()) => {
                    tracing::info!(device_id = %target, "renamed the session");
                    AccountOutcome::Renamed(Ok(()))
                }
                Err(e) => {
                    tracing::warn!(device_id = %target, "could not rename the session: {e}");
                    AccountOutcome::Renamed(Err(UserMessage::new(
                        UserMessageKind::SessionRenameFailed,
                    )))
                }
            }
        });
    }

    pub(super) fn sign_out_session(&mut self, port: Arc<dyn AccountPort>, device_id: String) {
        let Some(name) = self.other_session_name(&device_id) else {
            tracing::debug!(%device_id, "no other session of yours by that id");
            return;
        };
        if self.api().is_oauth() {
            self.open_page(port, AccountPage::SignOut(device_id));
            return;
        }
        self.start(port, Operation::SignOut { device_id, name }, None);
    }

    pub(super) fn start_password_change(&mut self) {
        if !self.may_change_password() || self.pending.is_some() {
            tracing::debug!("the password cannot be changed from here now");
            return;
        }
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        dialog.step = Step::ChangingPassword;
        dialog.error = UserMessage::default();
        dialog.notice = UserMessage::default();
        self.publish();
    }

    pub(super) fn change_password(
        &mut self,
        port: Arc<dyn AccountPort>,
        password: AccountPassword,
        sign_out_others: bool,
    ) {
        let changing = self
            .dialog
            .as_ref()
            .is_some_and(|dialog| matches!(dialog.step, Step::ChangingPassword));
        if !changing || !self.may_change_password() {
            tracing::debug!("dropping a new password typed into a closed form");
            return;
        }
        if !is_complex_enough(&password) {
            self.report(UserMessage::new(UserMessageKind::NewPasswordTooWeak));
            self.publish();
            return;
        }
        self.start(
            port,
            Operation::ChangePassword {
                password,
                sign_out_others,
            },
            None,
        );
    }

    pub(super) fn start_deactivation(&mut self, port: Arc<dyn AccountPort>) {
        if self.dialog.is_none() || self.pending.is_some() {
            return;
        }
        if self.api().is_oauth() {
            self.open_page(port, AccountPage::Deactivate);
            return;
        }
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        dialog.step = Step::ConfirmingDeactivation;
        dialog.error = UserMessage::default();
        dialog.notice = UserMessage::default();
        self.publish();
    }

    pub(super) fn deactivate(&mut self, port: Arc<dyn AccountPort>, erase: bool) {
        let confirming = self
            .dialog
            .as_ref()
            .is_some_and(|dialog| matches!(dialog.step, Step::ConfirmingDeactivation));
        if !confirming || self.api().is_oauth() {
            tracing::debug!("the account cannot be deactivated from here now");
            return;
        }
        self.start(port, Operation::Deactivate { erase }, None);
    }

    pub(super) fn open_account_page(&mut self, port: Arc<dyn AccountPort>, page: AccountPage) {
        if !matches!(self.api(), AccountApi::OAuth { pages: true }) {
            tracing::debug!(?page, "this homeserver offers no account pages");
            return;
        }
        self.open_page(port, page);
    }

    pub(super) fn awaits_auth(&self) -> bool {
        self.dialog
            .as_ref()
            .is_some_and(|dialog| matches!(dialog.step, Step::Authenticating { .. }))
    }

    pub(super) fn answer_auth(&mut self, port: Arc<dyn AccountPort>, reply: Reply) {
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

    pub(super) fn reopen_auth_page(&mut self) {
        let Some(Dialog {
            step: Step::Authenticating { round, .. },
            ..
        }) = self.dialog.as_ref()
        else {
            return;
        };
        if let Some(page) = round.page().cloned() {
            self.launch(page);
        }
    }

    pub(super) fn cancel_auth(&mut self) {
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

    pub(super) fn settled(
        &mut self,
        port: Option<Arc<dyn AccountPort>>,
        request: u64,
        outcome: AccountOutcome,
    ) -> Settled {
        if request < self.first_request_of_session {
            tracing::debug!(
                request,
                "dropping an account action of a session that ended"
            );
            return Settled::Nothing;
        }
        let Some(pending) = self.pending.take_if(|pending| pending.request == request) else {
            tracing::debug!(request, "dropping an account action nobody waits for");
            return Settled::Nothing;
        };
        let settled = match (pending.work, outcome) {
            (Work::Rename { .. }, AccountOutcome::Renamed(result)) => {
                if let Err(message) = result {
                    self.report(message);
                }
                Settled::Nothing
            }
            (Work::Guarded(operation), AccountOutcome::Guarded(result)) => {
                self.guarded_settled(port, operation, result)
            }
            (Work::Rename { .. }, AccountOutcome::Guarded(_))
            | (Work::Guarded(_), AccountOutcome::Renamed(_)) => Settled::Nothing,
        };
        self.publish();
        settled
    }

    pub(super) fn check_logout(&mut self, port: Arc<dyn AccountPort>) {
        if self.logout_check.is_some() || self.logout_warning.is_some() {
            tracing::debug!("the logout is already being checked or waits for an answer");
            return;
        }
        self.requests = self.requests.wrapping_add(1);
        let request = self.requests;
        self.logout_check = Some(request);
        let events = self.events.clone();
        self.actions.spawn(async move {
            let risks = port.logout_risks().await;
            drop(events.send(AppEvent::Account(AccountEvent::LogoutChecked {
                request,
                risks,
            })));
        });
    }

    pub(super) fn logout_checked(&mut self, request: u64, risks: LogoutRisks) -> LogoutDecision {
        if self.logout_check != Some(request) {
            return LogoutDecision::Wait;
        }
        self.logout_check = None;
        tracing::info!(
            keys_at_risk = risks.keys_at_risk,
            unsent = risks.unsent,
            "checked what logging out would lose"
        );
        if !risks.any() {
            return LogoutDecision::Proceed;
        }
        self.logout_warning = Some(risks);
        self.publish_logout();
        LogoutDecision::Wait
    }

    pub(super) fn confirm_logout(&mut self) -> bool {
        if self.logout_warning.take().is_none() {
            return false;
        }
        self.publish_logout();
        true
    }

    pub(super) fn cancel_logout(&mut self) {
        if self.logout_warning.take().is_some() {
            self.publish_logout();
        }
    }

    pub(super) async fn restart(&mut self) {
        tokio::join!(self.tasks.restart(), self.actions.restart());
        self.first_request_of_session = self.requests.wrapping_add(1);
        self.dialog = None;
        self.pending = None;
        self.logout_check = None;
        self.logout_warning = None;
    }

    pub(super) async fn shutdown(&mut self) {
        tokio::join!(self.tasks.shutdown(), self.actions.shutdown());
    }

    fn follow(&mut self, port: &Arc<dyn AccountPort>, generation: u64) {
        let facts_port = Arc::clone(port);
        let events = self.events.clone();
        let token = self.tasks.token();
        self.tasks.spawn(async move {
            let Some(facts) = token.run_until_cancelled(facts_port.facts()).await else {
                return;
            };
            drop(events.send(AppEvent::Account(AccountEvent::FactsRead {
                generation,
                facts,
            })));
        });

        let follow_port = Arc::clone(port);
        let events = self.events.clone();
        let token = self.tasks.token();
        self.tasks.spawn(async move {
            let (sessions_tx, sessions_rx) = mpsc::channel(SESSIONS_CHANNEL_CAP);
            let follow = async {
                tokio::join!(
                    follow_sessions(follow_port.as_ref(), sessions_tx),
                    forward_sessions(&events, generation, sessions_rx),
                )
            };
            token.run_until_cancelled(follow).await;
        });
    }

    fn start(
        &mut self,
        port: Arc<dyn AccountPort>,
        operation: Operation,
        auth: Option<AuthAnswer>,
    ) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        dialog.error = UserMessage::default();
        dialog.notice = UserMessage::default();
        let work = operation.clone();
        self.act(Work::Guarded(work), move || async move {
            let result = operation.run(port.as_ref(), auth.as_ref()).await;
            AccountOutcome::Guarded(result.map_err(|e| {
                tracing::warn!("an account action failed: {e}");
                operation.failure(&e)
            }))
        });
    }

    fn guarded_settled(
        &mut self,
        port: Option<Arc<dyn AccountPort>>,
        operation: Operation,
        result: Result<Guarded, UserMessage>,
    ) -> Settled {
        match result {
            Ok(Guarded::Done) => self.finished(operation),
            Ok(Guarded::NeedsAuth(challenge)) => {
                self.needs_auth(port, operation, *challenge);
                Settled::Nothing
            }
            Err(message) => {
                if let Some(dialog) = self.dialog.as_mut() {
                    dialog.step = operation.origin();
                }
                self.report(message);
                Settled::Nothing
            }
        }
    }

    fn finished(&mut self, operation: Operation) -> Settled {
        match operation {
            Operation::SignOut { device_id, name } => {
                self.signed_out(&device_id, &name);
                Settled::Nothing
            }
            Operation::ChangePassword {
                sign_out_others, ..
            } => {
                self.password_changed(sign_out_others);
                Settled::Nothing
            }
            Operation::Deactivate { erase } => {
                tracing::info!(erase, "the homeserver deactivated the account");
                self.dialog = None;
                Settled::Deactivated
            }
        }
    }

    fn signed_out(&mut self, device_id: &str, name: &str) {
        tracing::info!(%device_id, "signed the session out");
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        dialog.step = Step::Overview;
        dialog.notice = UserMessage::about(UserMessageKind::SessionSignedOut, &name);
        if let Some(sessions) = dialog.sessions.as_mut() {
            sessions
                .others
                .retain(|session| session.device_id != device_id);
        }
    }

    fn password_changed(&mut self, sign_out_others: bool) {
        tracing::info!(sign_out_others, "changed the account's password");
        let kind = if sign_out_others {
            UserMessageKind::PasswordChangedOthersSignedOut
        } else {
            UserMessageKind::PasswordChanged
        };
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.step = Step::Overview;
            dialog.notice = UserMessage::new(kind);
        }
    }

    fn needs_auth(
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
                    self.launch(page.clone());
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

    fn set_step(&mut self, step: Step) {
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.step = step;
        }
    }

    fn open_page(&mut self, port: Arc<dyn AccountPort>, page: AccountPage) {
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

    fn launch(&mut self, page: LauncherSafeUrl) {
        let browser = Arc::clone(&self.browser);
        tracing::info!("opening the homeserver's page for the step it asks for");
        self.actions.spawn(async move {
            if let Err(e) = browser.open_url(&page).await {
                tracing::warn!("the homeserver's page could not be opened: {e}");
            }
        });
    }

    fn act<F, Fut>(&mut self, work: Work, run: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = AccountOutcome> + Send,
    {
        if self.pending.is_some() {
            tracing::debug!("an account action is already running");
            return;
        }
        self.requests = self.requests.wrapping_add(1);
        let request = self.requests;
        self.pending = Some(Pending { request, work });
        self.publish();
        let events = self.events.clone();
        self.actions.spawn(async move {
            let outcome = run().await;
            drop(events.send(AppEvent::Account(AccountEvent::Settled {
                request,
                outcome,
            })));
        });
    }

    fn current_dialog(&mut self, generation: u64) -> Option<&mut Dialog> {
        self.dialog
            .as_mut()
            .filter(|dialog| dialog.generation == generation)
    }

    fn api(&self) -> AccountApi {
        self.dialog
            .as_ref()
            .and_then(|dialog| dialog.facts.as_ref())
            .map(|facts| facts.api)
            .unwrap_or_default()
    }

    fn may_change_password(&self) -> bool {
        self.dialog
            .as_ref()
            .and_then(|dialog| dialog.facts.as_ref())
            .is_some_and(|facts| !facts.api.is_oauth() && facts.capabilities.change_password)
    }

    fn knows_session(&self, device_id: &str) -> bool {
        self.dialog
            .as_ref()
            .and_then(|dialog| dialog.sessions.as_ref())
            .is_some_and(|sessions| sessions.names(device_id))
    }

    fn other_session_name(&self, device_id: &str) -> Option<String> {
        let sessions = self.dialog.as_ref()?.sessions.as_ref()?;
        sessions
            .other(device_id)
            .map(|session| session.shown_name().to_owned())
    }

    fn report(&mut self, message: UserMessage) {
        match self.dialog.as_mut() {
            Some(dialog) => dialog.error = message,
            None => show_toast(self.output.as_ref(), Toast::Error(message)),
        }
    }

    fn publish(&self) {
        let view = self.dialog.as_ref().map(|dialog| self.dialog_view(dialog));
        self.output
            .publish(Box::new(move |state| state.account.dialog = view));
    }

    fn dialog_view(&self, dialog: &Dialog) -> AccountDialog {
        let (activity, busy_session) = match self.pending.as_ref().map(|pending| &pending.work) {
            None => (AccountActivity::None, None),
            Some(Work::Rename { device_id }) => {
                (AccountActivity::Renaming, Some(device_id.clone()))
            }
            Some(Work::Guarded(operation)) => (
                operation.activity(),
                operation.device().map(ToOwned::to_owned),
            ),
        };
        let busy_session = busy_session.or_else(|| match &dialog.step {
            Step::Authenticating { operation, .. } => operation.device().map(ToOwned::to_owned),
            Step::Overview | Step::ChangingPassword | Step::ConfirmingDeactivation => None,
        });
        AccountDialog {
            step: match &dialog.step {
                Step::Overview => AccountStep::Overview,
                Step::ChangingPassword => AccountStep::ChangingPassword,
                Step::ConfirmingDeactivation => AccountStep::ConfirmingDeactivation,
                Step::Authenticating { operation, round } if round.is_silent() => {
                    operation.origin_step()
                }
                Step::Authenticating { operation, round } => {
                    AccountStep::Authenticating(AccountAuth {
                        prompt: round.prompt(),
                        guarding: operation.activity(),
                        subject: operation.subject(),
                    })
                }
            },
            activity,
            busy_session,
            sessions: dialog.sessions.clone(),
            facts: dialog.facts.clone(),
            error: dialog.error.clone(),
            notice: dialog.notice.clone(),
        }
    }

    fn publish_logout(&self) {
        let view = LogoutView {
            warning: self.logout_warning,
        };
        self.output
            .publish(Box::new(move |state| state.logout = view));
    }
}

async fn follow_sessions(port: &dyn AccountPort, sessions_tx: mpsc::Sender<OwnSessions>) {
    if let Err(e) = port.follow_sessions(sessions_tx).await {
        tracing::warn!("failed to follow this account's sessions: {e}");
    }
}

async fn forward_sessions(
    events: &EventSender,
    generation: u64,
    mut sessions_rx: mpsc::Receiver<OwnSessions>,
) {
    while let Some(sessions) = sessions_rx.recv().await {
        let event = AppEvent::Account(AccountEvent::SessionsChanged {
            generation,
            sessions,
        });
        if events.send(event).is_err() {
            return;
        }
    }
}
