use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

use super::establish::EstablishedSession;
use super::event::{AppEvent, EndReason, SessionEvent};
use super::recover::Recovery;
use super::task_group::{self, TaskGroup};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{LoginActivity, LoginStep};
use crate::domain::account::AccountScope;
use crate::domain::models::{LoginCredentials, LoginMethod, ServerInfo, Session};
use crate::error::{AppError, AuthFailure, Result};
use crate::ports::browser::BrowserPort;
use crate::ports::matrix::{
    AuthPort, AuthenticatedSession, CleanupReport, RestoreStep, SessionPort,
};
use crate::ports::output::AppOutputPort;
use crate::ports::storage::{StoragePort, StoredSession};
use crate::util::random_hex;

const OAUTH_CALLBACK_TIMEOUT: Duration = Duration::from_mins(5);
const LOCAL_ERASURE_RESERVE: Duration = Duration::from_secs(1);
const SERVER_LOGOUT_TIMEOUT: Duration =
    task_group::SHUTDOWN_GRACE.saturating_sub(LOCAL_ERASURE_RESERVE);
const PERSIST_RETRY_FLOOR: Duration = Duration::from_secs(1);
const PERSIST_RETRY_CEILING: Duration = Duration::from_mins(1);

fn generate_passphrase() -> String {
    random_hex(32)
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

fn login_failure(err: &AppError) -> UserMessageKind {
    let AppError::Auth { kind, .. } = err else {
        return UserMessageKind::LoginFailed;
    };
    match kind {
        AuthFailure::Unreachable => UserMessageKind::ServerUnreachable,
        AuthFailure::InvalidCredentials => UserMessageKind::InvalidCredentials,
        AuthFailure::AccountDeactivated => UserMessageKind::AccountDeactivated,
        AuthFailure::InvalidUsername => UserMessageKind::InvalidUsername,
        AuthFailure::RateLimited => UserMessageKind::RateLimited,
        AuthFailure::MethodUnsupported => UserMessageKind::LoginMethodUnsupported,
        AuthFailure::IdentityDiverged => UserMessageKind::IdentityDiverged,
        AuthFailure::Unknown => UserMessageKind::LoginFailed,
    }
}

fn restore_failure(err: &AppError) -> UserMessageKind {
    if let AppError::Auth {
        kind: AuthFailure::IdentityDiverged,
        ..
    } = err
    {
        return UserMessageKind::IdentityDiverged;
    }
    UserMessageKind::SessionRestoreFailed
}

fn restore_activity(step: RestoreStep) -> LoginActivity {
    match step {
        RestoreStep::Connecting => LoginActivity::Connecting,
        RestoreStep::RestoringAuth => LoginActivity::RestoringAuth,
    }
}

async fn end_server_session(lifecycle_port: &dyn SessionPort) {
    match timeout(SERVER_LOGOUT_TIMEOUT, lifecycle_port.logout()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("failed to logout from server: {e}"),
        Err(_) => tracing::warn!("server logout timed out, erasing local state anyway"),
    }
}

fn classify_unusable_session(loaded: Result<StoredSession>) -> (&'static str, Option<AppError>) {
    match loaded {
        Ok(StoredSession::Absent) => ("no saved session found", None),
        Ok(StoredSession::Incomplete) => ("saved session incomplete, re-login required", None),
        Ok(StoredSession::CredentialsUnavailable(e)) => ("credential store unavailable", Some(e)),
        Err(e) => ("failed to load session", Some(e)),
        Ok(StoredSession::Present(_)) => ("session present", None),
    }
}

#[derive(Clone)]
struct SessionTasks {
    auth: Arc<dyn AuthPort>,
    storage: Arc<dyn StoragePort>,
    browser: Arc<dyn BrowserPort>,
    events: mpsc::UnboundedSender<AppEvent>,
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
        events: mpsc::UnboundedSender<AppEvent>,
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

    pub(super) fn cancel_oauth(&mut self) {
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

    pub(super) fn spawn_logout(
        &self,
        group: &mut TaskGroup,
        session: u64,
        account: AccountScope,
        lifecycle_port: Arc<dyn SessionPort>,
    ) {
        let tasks = self.tasks.clone();
        group.spawn(async move { tasks.logout(session, &account, lifecycle_port).await });
    }

    pub(super) fn spawn_expire_session(
        &self,
        group: &mut TaskGroup,
        session: u64,
        account: AccountScope,
        lifecycle_port: Arc<dyn SessionPort>,
    ) {
        let tasks = self.tasks.clone();
        group.spawn(async move {
            tasks
                .expire_session(session, &account, lifecycle_port)
                .await;
        });
    }

    pub(super) fn spawn_session_persister(
        &self,
        group: &mut TaskGroup,
        lifecycle_port: Arc<dyn SessionPort>,
    ) {
        let storage = Arc::clone(&self.tasks.storage);
        let events = self.tasks.events.clone();
        let token = group.token();
        group.spawn(async move {
            let (session_tx, mut session_rx) = mpsc::unbounded_channel::<Session>();

            let listen = async move {
                tokio::select! {
                    result = lifecycle_port.subscribe_session_changes(session_tx) => {
                        match result {
                            Ok(()) => tracing::debug!("session change listener ended"),
                            Err(e) => tracing::warn!("session change listener failed: {e}"),
                        }
                    }
                    () = token.cancelled() => {
                        tracing::debug!("session change listener cancelled");
                    }
                }
            };

            let persist = async move {
                let mut persister = SessionPersister::new(storage, events);
                let mut dirty: Option<Session> = None;
                let mut retry_in = None;

                loop {
                    match next_persist_step(&mut session_rx, retry_in).await {
                        PersistStep::Refreshed(session) => dirty = Some(session),
                        PersistStep::Retry => {}
                        PersistStep::Closed => break,
                    }
                    let Some(session) = dirty.as_ref() else {
                        continue;
                    };
                    retry_in = persister.store(session).await;
                    if retry_in.is_none() {
                        dirty = None;
                    } else {
                        persister.report_failure_once();
                    }
                }

                if let Some(session) = dirty {
                    persister.flush(&session).await;
                }
            };

            tokio::join!(listen, persist);
        });
    }

    pub(super) fn spawn_user_avatar_fetch(
        &self,
        group: &mut TaskGroup,
        lifecycle_port: Arc<dyn SessionPort>,
    ) {
        let events = self.tasks.events.clone();
        group.spawn(async move {
            match lifecycle_port.fetch_user_avatar().await {
                Ok(path) => send(&events, SessionEvent::UserAvatar(path)),
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

    pub(super) fn show_login(&self, message: Option<UserMessage>) {
        self.output.publish(Box::new(move |view| {
            view.lifecycle.step = LoginStep::Homeserver;
            view.lifecycle.activity = LoginActivity::Idle;
            if let Some(message) = message {
                view.lifecycle.messages = vec![message];
            }
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

fn send(events: &mpsc::UnboundedSender<AppEvent>, event: SessionEvent) {
    if events.send(AppEvent::Session(event)).is_err() {
        tracing::debug!("the app event loop is gone; dropping a session event");
    }
}

impl SessionTasks {
    fn send(&self, event: SessionEvent) {
        send(&self.events, event);
    }

    async fn restore_session(&self) {
        if let Some(capability) = self.try_restore_session().await {
            self.send(SessionEvent::Restored(Box::new(capability)));
        }
    }

    async fn try_restore_session(&self) -> Option<AuthenticatedSession> {
        self.send(SessionEvent::RestoreProgress(LoginActivity::LoadingSession));
        let session = self.load_saved_session().await?;
        let account = AccountScope::from_session(&session);

        self.send(SessionEvent::RestoreProgress(LoginActivity::OpeningStore));
        let passphrase = self.stored_passphrase(&account).await?;

        match self.restore_matrix_session(&session, &passphrase).await {
            Ok(capability) => Some(capability),
            Err(e) => {
                tracing::warn!("session restore failed, preserving local data: {e}");
                self.report_restore_failed(Some(restore_failure(&e)))
            }
        }
    }

    fn report_restore_failed<T>(&self, kind: Option<UserMessageKind>) -> Option<T> {
        self.send(SessionEvent::RestoreFailed(kind.map(UserMessage::new)));
        None
    }

    async fn load_saved_session(&self) -> Option<Session> {
        match self.storage.load_session().await {
            Ok(StoredSession::Present(session)) => {
                tracing::info!(user_id = %session.user_id, "found saved session");
                Some(session)
            }
            unusable => {
                let (reason, error) = classify_unusable_session(unusable);
                let Some(e) = error else {
                    tracing::info!("{reason}, showing login");
                    return self.report_restore_failed(None);
                };
                tracing::warn!("{reason}, preserving local data: {e}");
                self.report_restore_failed(Some(UserMessageKind::SessionUnreadable))
            }
        }
    }

    async fn stored_passphrase(&self, account: &AccountScope) -> Option<String> {
        match self.storage.load_passphrase(account).await {
            Ok(Some(passphrase)) => Some(passphrase),
            Ok(None) => {
                tracing::warn!("no store key for the saved session, preserving local data");
                self.report_restore_failed(Some(UserMessageKind::StoreKeyMissing))
            }
            Err(e) => {
                tracing::warn!("failed to read the store key: {e}");
                self.report_restore_failed(Some(UserMessageKind::StoreKeyUnreadable))
            }
        }
    }

    async fn restore_matrix_session(
        &self,
        session: &Session,
        passphrase: &str,
    ) -> Result<AuthenticatedSession> {
        let events = self.events.clone();
        let on_progress = Box::new(move |step| {
            send(
                &events,
                SessionEvent::RestoreProgress(restore_activity(step)),
            );
        });

        self.auth
            .restore_session(session, passphrase, on_progress)
            .await
    }

    async fn check_server(&self, homeserver: &str, passphrase: &str, attempt: u64) {
        tracing::info!(homeserver, "checking server");
        match self.auth.discover_auth(homeserver, passphrase).await {
            Ok(info) if info.auth_methods.is_empty() => {
                let flows = info.unsupported_flows.join(", ");
                tracing::warn!(homeserver, flows = %flows, "no supported login method");
                self.send(SessionEvent::AuthRejected {
                    attempt,
                    message: UserMessage::about(UserMessageKind::UnsupportedLoginMethod, &flows),
                });
            }
            Ok(info) => self.send(SessionEvent::ServerDiscovered {
                attempt,
                info: Box::new(info),
            }),
            Err(e) => {
                tracing::warn!(homeserver, "server discovery failed: {e}");
                self.reject(attempt, UserMessageKind::ServerUnreachable);
            }
        }
    }

    async fn login_password(
        &self,
        creds: LoginCredentials,
        passphrase: Option<String>,
        attempt: u64,
    ) {
        let outcome = match self.auth.login_password(creds).await {
            Ok(session) => self.establish_session(session, passphrase).await,
            Err(e) => Err(e),
        };
        match outcome {
            Ok(established) => self.send(SessionEvent::LoggedIn {
                attempt,
                established: Box::new(established),
            }),
            Err(e) => {
                tracing::warn!("password login failed: {e}");
                self.reject(attempt, login_failure(&e));
            }
        }
    }

    async fn login_oauth(
        &self,
        cancel: CancellationToken,
        passphrase: Option<String>,
        attempt: u64,
    ) {
        let result = self.run_oauth_flow(&cancel, passphrase, attempt).await;
        self.auth.cancel_oauth().await;
        match result {
            Ok(Some(established)) => self.send(SessionEvent::LoggedIn {
                attempt,
                established: Box::new(established),
            }),
            Ok(None) => {
                tracing::info!("OAuth login cancelled");
                self.send(SessionEvent::AuthCancelled { attempt });
            }
            Err(e) => {
                tracing::warn!("OAuth login failed: {e}");
                self.reject(attempt, login_failure(&e));
            }
        }
    }

    async fn run_oauth_flow(
        &self,
        cancel: &CancellationToken,
        passphrase: Option<String>,
        attempt: u64,
    ) -> Result<Option<EstablishedSession>> {
        let Some(session) = self.cancellable_browser_sign_in(cancel, attempt).await? else {
            return Ok(None);
        };
        self.send(SessionEvent::AuthActivity {
            attempt,
            activity: LoginActivity::OpeningStore,
        });
        self.establish_session(session, passphrase).await.map(Some)
    }

    async fn cancellable_browser_sign_in(
        &self,
        cancel: &CancellationToken,
        attempt: u64,
    ) -> Result<Option<Session>> {
        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(None),
            result = self.browser_sign_in(attempt) => result.map(Some),
        }
    }

    async fn browser_sign_in(&self, attempt: u64) -> Result<Session> {
        let oauth_data = self.auth.login_oauth_start().await?;
        self.browser.open_url(&oauth_data.auth_url).await?;
        self.send(SessionEvent::AuthActivity {
            attempt,
            activity: LoginActivity::WaitingAuth,
        });
        timeout(OAUTH_CALLBACK_TIMEOUT, self.auth.login_oauth_finish())
            .await
            .map_err(|_| AppError::Other("Timed out waiting for browser sign-in.".into()))?
    }

    async fn establish_session(
        &self,
        session: Session,
        passphrase: Option<String>,
    ) -> Result<EstablishedSession> {
        let passphrase = passphrase.ok_or_else(|| {
            AppError::Other("No login store was prepared. Please start again.".into())
        })?;

        let account = AccountScope::from_session(&session);
        let adoption = self.auth.adopt_session(&session, &passphrase).await?;
        EstablishedSession::record_or_roll_back(
            adoption,
            Arc::clone(&self.storage),
            account,
            &session,
            &passphrase,
        )
        .await
    }

    fn reject(&self, attempt: u64, kind: UserMessageKind) {
        self.send(SessionEvent::AuthRejected {
            attempt,
            message: UserMessage::new(kind),
        });
    }

    async fn expire_session(
        &self,
        session: u64,
        account: &AccountScope,
        lifecycle_port: Arc<dyn SessionPort>,
    ) {
        let report = self
            .clear_local_state(session, account, lifecycle_port.as_ref())
            .await;
        self.send(SessionEvent::LocalStateCleared {
            session,
            reason: EndReason::Expired,
            report,
        });
    }

    async fn logout(
        &self,
        session: u64,
        account: &AccountScope,
        lifecycle_port: Arc<dyn SessionPort>,
    ) {
        tracing::info!("user initiated logout");
        end_server_session(lifecycle_port.as_ref()).await;
        let report = self
            .clear_local_state(session, account, lifecycle_port.as_ref())
            .await;
        self.send(SessionEvent::LocalStateCleared {
            session,
            reason: EndReason::UserLogout,
            report,
        });
    }

    async fn clear_local_state(
        &self,
        session: u64,
        account: &AccountScope,
        lifecycle_port: &dyn SessionPort,
    ) -> CleanupReport {
        self.send(SessionEvent::ErasingLocalState { session });

        let mut report = CleanupReport::default();
        if let Err(e) = self.storage.clear_passphrase(account).await {
            report.fail(format!("the local store key could not be destroyed ({e})"));
        }
        if let Err(e) = self.storage.clear_session().await {
            report.fail(e.to_string());
        }
        report.merge(lifecycle_port.clear_store().await);
        report
    }
}

struct SessionPersister {
    storage: Arc<dyn StoragePort>,
    events: mpsc::UnboundedSender<AppEvent>,
    retry_in: Duration,
    reported: bool,
}

impl SessionPersister {
    fn new(storage: Arc<dyn StoragePort>, events: mpsc::UnboundedSender<AppEvent>) -> Self {
        Self {
            storage,
            events,
            retry_in: PERSIST_RETRY_FLOOR,
            reported: false,
        }
    }

    async fn store(&mut self, session: &Session) -> Option<Duration> {
        match self.storage.save_session(session).await {
            Ok(()) => {
                if self.reported {
                    tracing::info!("persisted the session tokens after earlier failures");
                } else {
                    tracing::info!("persisted the current session tokens");
                }
                self.retry_in = PERSIST_RETRY_FLOOR;
                self.reported = false;
                None
            }
            Err(e) => {
                let retry_in = self.retry_in;
                self.retry_in = retry_in.saturating_mul(2).min(PERSIST_RETRY_CEILING);
                tracing::warn!(
                    retry_in = retry_in.as_secs(),
                    "failed to persist refreshed session, retrying: {e}"
                );
                Some(retry_in)
            }
        }
    }

    fn report_failure_once(&mut self) {
        if self.reported {
            return;
        }
        self.reported = true;
        send(&self.events, SessionEvent::TokensNotPersisted);
    }

    async fn flush(&mut self, session: &Session) {
        if self.store(session).await.is_some() {
            tracing::warn!("the newest session tokens were not persisted before shutting down");
        }
    }
}

enum PersistStep {
    Refreshed(Session),
    Retry,
    Closed,
}

async fn next_persist_step(
    session_rx: &mut mpsc::UnboundedReceiver<Session>,
    retry_in: Option<Duration>,
) -> PersistStep {
    let Some(delay) = retry_in else {
        return recv_newest_session(session_rx).await;
    };
    tokio::select! {
        biased;
        step = recv_newest_session(session_rx) => step,
        () = sleep(delay) => PersistStep::Retry,
    }
}

async fn recv_newest_session(session_rx: &mut mpsc::UnboundedReceiver<Session>) -> PersistStep {
    let Some(mut newest) = session_rx.recv().await else {
        return PersistStep::Closed;
    };
    while let Ok(newer) = session_rx.try_recv() {
        newest = newer;
    }
    PersistStep::Refreshed(newest)
}
