mod persister;
mod tasks;

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use self::persister::{SessionPersister, listen_for_session_changes};
use self::tasks::SessionTasks;
use super::event::{AppEvent, EndReason, SessionEvent};
use super::input::EventSender;
use super::recover::Recovery;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{LoginActivity, LoginStep};
use crate::domain::account::AccountScope;
use crate::domain::auth::{LoginCredentials, LoginMethod, ServerInfo, Session};
use crate::domain::link::LauncherSafeUrl;
use crate::ports::browser::BrowserPort;
use crate::ports::matrix::{AuthPort, CleanupReport, DeviceRemoval, SessionPort};
use crate::ports::output::AppOutputPort;
use crate::ports::storage::StoragePort;
use crate::util::random_hex;

const PASSPHRASE_BYTES: usize = 32;

fn generate_passphrase() -> String {
    random_hex(PASSPHRASE_BYTES)
}

pub(super) fn cleanup_problem(report: &CleanupReport) -> Option<UserMessage> {
    if report.is_clean() {
        return None;
    }
    tracing::warn!(
        "local account data was not fully erased: {}",
        report.summary()
    );
    if report.is_quarantined_only() {
        return Some(UserMessage::about(
            UserMessageKind::DataQuarantined,
            &report.quarantined_paths(),
        ));
    }
    Some(UserMessage::new(UserMessageKind::DataNotErased))
}

pub(super) fn unconfirmed_device(removal: DeviceRemoval, device_id: &str) -> Option<String> {
    match removal {
        DeviceRemoval::Confirmed => None,
        DeviceRemoval::Unconfirmed => Some(device_id.to_owned()),
    }
}

pub(super) fn left_behind_notice(device_id: &str) -> UserMessage {
    UserMessage::about(UserMessageKind::DeviceLeftBehind, &device_id)
}

pub(super) struct SuspendedSession {
    pub(super) session: Session,
    pub(super) account: AccountScope,
    pub(super) lifecycle: Arc<dyn SessionPort>,
}

pub(super) struct EndingSession {
    pub(super) account: AccountScope,
    pub(super) device_id: String,
    pub(super) lifecycle: Arc<dyn SessionPort>,
}

pub(super) struct SessionController {
    tasks: SessionTasks,
    output: Arc<dyn AppOutputPort>,
    pending_passphrase: Option<String>,
    oauth_cancel: Option<CancellationToken>,
}

impl SessionController {
    pub(super) fn new(
        auth: Arc<dyn AuthPort>,
        storage: Arc<dyn StoragePort>,
        browser: Arc<dyn BrowserPort>,
        output: Arc<dyn AppOutputPort>,
        events: EventSender,
    ) -> Self {
        Self {
            tasks: SessionTasks {
                auth,
                storage,
                browser,
                events,
            },
            output,
            pending_passphrase: None,
            oauth_cancel: None,
        }
    }

    pub(super) async fn recover_interrupted_logins(&self) -> Recovery {
        super::recover::recover_interrupted_logins(
            self.tasks.auth.as_ref(),
            self.tasks.storage.as_ref(),
        )
        .await
    }

    pub(super) fn spawn_restore_session(&self, group: &mut TaskGroup) {
        self.output.publish(Box::new(|view| {
            view.lifecycle.step = LoginStep::Loading;
            view.lifecycle.messages.clear();
        }));
        let tasks = self.tasks.clone();
        group.spawn(async move { tasks.restore_session().await });
    }

    pub(super) fn spawn_check_server(
        &mut self,
        group: &mut TaskGroup,
        homeserver: String,
        attempt: u64,
    ) {
        self.begin_login(LoginActivity::CheckingServer);
        let passphrase = generate_passphrase();
        self.pending_passphrase = Some(passphrase.clone());
        let tasks = self.tasks.clone();
        group.spawn(async move { tasks.check_server(&homeserver, &passphrase, attempt).await });
    }

    pub(super) fn spawn_login_password(
        &mut self,
        group: &mut TaskGroup,
        creds: LoginCredentials,
        attempt: u64,
    ) {
        self.begin_login(LoginActivity::LoggingIn);
        let passphrase = self.pending_passphrase.clone();
        let tasks = self.tasks.clone();
        group.spawn(async move { tasks.login_password(creds, passphrase, attempt).await });
    }

    pub(super) fn spawn_login_oauth(&mut self, group: &mut TaskGroup, attempt: u64) {
        self.begin_login(LoginActivity::OpeningBrowser);
        let cancel = CancellationToken::new();
        self.oauth_cancel = Some(cancel.clone());
        let passphrase = self.pending_passphrase.clone();
        let tasks = self.tasks.clone();
        group.spawn(async move { tasks.login_oauth(cancel, passphrase, attempt).await });
    }

    pub(super) fn spawn_reauth_password(
        &mut self,
        group: &mut TaskGroup,
        prior: Session,
        password: String,
        attempt: u64,
    ) {
        self.begin_login(LoginActivity::LoggingIn);
        let tasks = self.tasks.clone();
        group.spawn(async move { tasks.reauth_password(prior, password, attempt).await });
    }

    pub(super) fn spawn_reauth_oauth(
        &mut self,
        group: &mut TaskGroup,
        prior: Session,
        attempt: u64,
    ) {
        self.begin_login(LoginActivity::OpeningBrowser);
        let cancel = CancellationToken::new();
        self.oauth_cancel = Some(cancel.clone());
        let tasks = self.tasks.clone();
        group.spawn(async move { tasks.reauth_oauth(prior, cancel, attempt).await });
    }

    pub(super) fn spawn_open_link(&self, group: &mut TaskGroup, url: LauncherSafeUrl) {
        let browser = Arc::clone(&self.tasks.browser);
        group.spawn(async move {
            if let Err(e) = browser.open_url(&url).await {
                tracing::warn!("a message link could not be opened: {e}");
            }
        });
    }

    pub(super) fn cancel_oauth(&mut self) {
        self.set_activity(LoginActivity::Cancelling);
        if let Some(token) = self.oauth_cancel.take() {
            tracing::info!("cancelling OAuth login");
            token.cancel();
        }
    }

    pub(super) fn finish_oauth(&mut self) {
        self.oauth_cancel = None;
    }

    pub(super) fn spend_pending_passphrase(&mut self) {
        self.pending_passphrase = None;
    }

    pub(super) fn spawn_logout(&self, group: &mut TaskGroup, session: u64, ending: EndingSession) {
        let tasks = self.tasks.clone();
        let quit = group.token();
        group.spawn(async move { tasks.logout(session, &ending, &quit).await });
    }

    pub(super) fn spawn_erase_session(
        &self,
        group: &mut TaskGroup,
        session: u64,
        ending: EndingSession,
        reason: EndReason,
        left_behind: Option<String>,
    ) {
        let tasks = self.tasks.clone();
        group.spawn(async move {
            tasks
                .erase_session(session, &ending, reason, left_behind)
                .await;
        });
    }

    pub(super) fn spawn_session_persister(
        &self,
        group: &mut TaskGroup,
        lifecycle_port: Arc<dyn SessionPort>,
        session: u64,
    ) {
        let persister = SessionPersister::new(
            Arc::clone(&self.tasks.storage),
            self.tasks.events.clone(),
            session,
        );
        let token = group.token();
        group.spawn(async move {
            let (session_tx, session_rx) = mpsc::unbounded_channel::<Session>();
            tokio::join!(
                listen_for_session_changes(lifecycle_port, session_tx, token),
                persister.run(session_rx),
            );
        });
    }

    pub(super) fn spawn_user_avatar_fetch(
        &self,
        group: &mut TaskGroup,
        lifecycle_port: Arc<dyn SessionPort>,
        session: u64,
    ) {
        let events = self.tasks.events.clone();
        group.spawn(async move {
            match lifecycle_port.fetch_user_avatar().await {
                Ok(path) => send(&events, SessionEvent::UserAvatar { session, path }),
                Err(e) => tracing::debug!("user avatar fetch failed: {e}"),
            }
        });
    }

    pub(super) fn back_to_homeserver(&self) {
        self.output.publish(Box::new(|view| {
            view.lifecycle.step = LoginStep::Homeserver;
            view.lifecycle.activity = LoginActivity::Idle;
            view.lifecycle.messages.clear();
        }));
    }

    pub(super) fn begin_login(&self, activity: LoginActivity) {
        self.output.publish(Box::new(move |view| {
            view.lifecycle.activity = activity;
            view.lifecycle.messages.clear();
        }));
    }

    pub(super) fn set_activity(&self, activity: LoginActivity) {
        self.output
            .publish(Box::new(move |view| view.lifecycle.activity = activity));
    }

    pub(super) fn show_credentials(&self, info: ServerInfo) {
        let method = LoginMethod::from_auth_methods(&info.auth_methods);
        self.output.publish(Box::new(move |view| {
            view.lifecycle.method = method;
            view.lifecycle.resolved_homeserver = info.homeserver_url;
            view.lifecycle.step = LoginStep::Credentials;
            view.lifecycle.activity = LoginActivity::Idle;
        }));
    }

    pub(super) fn show_login(&self, messages: Vec<UserMessage>) {
        self.output.publish(Box::new(move |view| {
            view.lifecycle.step = LoginStep::Homeserver;
            view.lifecycle.activity = LoginActivity::Idle;
            if !messages.is_empty() {
                view.lifecycle.messages = messages;
            }
        }));
    }

    pub(super) fn show_restore_paused(&self, message: UserMessage) {
        self.output.publish(Box::new(move |view| {
            view.lifecycle.step = LoginStep::RestorePaused;
            view.lifecycle.activity = LoginActivity::Idle;
            view.lifecycle.messages = vec![message];
        }));
    }

    pub(super) fn fail_login(&self, messages: Vec<UserMessage>) {
        self.output.publish(Box::new(move |view| {
            view.lifecycle.activity = LoginActivity::Idle;
            view.lifecycle.messages = messages;
        }));
    }

    pub(super) fn settle_logout(&self, messages: Vec<UserMessage>) {
        self.output.publish(Box::new(move |view| {
            view.lifecycle.activity = LoginActivity::Idle;
            if !messages.is_empty() {
                view.lifecycle.messages = messages;
            }
        }));
    }
}

fn send(events: &EventSender, event: SessionEvent) {
    drop(events.send(AppEvent::Session(event)));
}
