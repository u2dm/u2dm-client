use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::establish::EstablishedSession;
use super::lifecycle::Lifecycle;
use super::task_group::{self, TaskGroup};
use crate::commands::{LoginActivity, LoginStep, Toast, UserMessage, UserMessageKind};
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

pub(super) enum AuthOutcome {
    Login {
        attempt: u64,
        established: EstablishedSession,
    },
    Restore(AuthenticatedSession),
}

fn generate_passphrase() -> String {
    random_hex(32)
}

fn cleanup_problem(report: &CleanupReport) -> Option<UserMessage> {
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
        AuthFailure::Unknown => UserMessageKind::LoginFailed,
    }
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
pub(super) struct SessionController {
    auth: Arc<dyn AuthPort>,
    storage: Arc<dyn StoragePort>,
    browser: Arc<dyn BrowserPort>,
    output: Arc<dyn AppOutputPort>,
    lifecycle: Lifecycle,
    auth_tx: mpsc::UnboundedSender<AuthOutcome>,
    oauth_cancel: Arc<StdMutex<Option<CancellationToken>>>,
    pending_passphrase: Arc<StdMutex<Option<String>>>,
}

impl SessionController {
    pub(super) fn new(
        auth: Arc<dyn AuthPort>,
        storage: Arc<dyn StoragePort>,
        browser: Arc<dyn BrowserPort>,
        output: Arc<dyn AppOutputPort>,
        lifecycle: Lifecycle,
        auth_tx: mpsc::UnboundedSender<AuthOutcome>,
    ) -> Self {
        Self {
            auth,
            storage,
            browser,
            output,
            lifecycle,
            auth_tx,
            oauth_cancel: Arc::new(StdMutex::new(None)),
            pending_passphrase: Arc::new(StdMutex::new(None)),
        }
    }

    pub(super) fn spawn_restore_session(&self, group: &mut TaskGroup) {
        let this = self.clone();
        group.spawn(async move { this.restore_session().await });
    }

    pub(super) fn spawn_check_server(
        &self,
        group: &mut TaskGroup,
        homeserver: String,
        attempt: u64,
    ) {
        self.begin_login(LoginActivity::CheckingServer);
        let this = self.clone();
        group.spawn(async move { this.check_server(&homeserver, attempt).await });
    }

    pub(super) fn spawn_login_password(
        &self,
        group: &mut TaskGroup,
        creds: LoginCredentials,
        attempt: u64,
    ) {
        self.begin_login(LoginActivity::LoggingIn);
        let this = self.clone();
        group.spawn(async move { this.login_password(creds, attempt).await });
    }

    pub(super) fn spawn_login_oauth(&self, group: &mut TaskGroup, attempt: u64) {
        self.begin_login(LoginActivity::OpeningBrowser);
        let cancel = self.begin_oauth();
        let this = self.clone();
        group.spawn(async move { this.login_oauth(cancel, attempt).await });
    }

    pub(super) fn back_to_homeserver(&self) {
        self.output.publish(Box::new(|view| {
            view.lifecycle.step = LoginStep::Homeserver;
            view.lifecycle.activity = LoginActivity::Idle;
            view.lifecycle.messages.clear();
        }));
    }

    pub(super) fn cancel_oauth(&self) {
        let Ok(mut guard) = self.oauth_cancel.lock() else {
            return;
        };
        if let Some(token) = guard.take() {
            tracing::info!("cancelling OAuth login");
            token.cancel();
        }
    }

    pub(super) fn spawn_logout(
        &self,
        group: &mut TaskGroup,
        session: u64,
        account: AccountScope,
        lifecycle_port: Arc<dyn SessionPort>,
    ) {
        let this = self.clone();
        group.spawn(async move { this.logout(session, &account, lifecycle_port).await });
    }

    pub(super) fn spawn_expire_session(
        &self,
        group: &mut TaskGroup,
        session: u64,
        account: AccountScope,
        lifecycle_port: Arc<dyn SessionPort>,
    ) {
        let this = self.clone();
        group.spawn(async move { this.expire_session(session, &account, lifecycle_port).await });
    }

    async fn restore_session(&self) {
        let Some(capability) = self.try_restore_session().await else {
            self.lifecycle.restore_failed();
            return;
        };
        self.send_auth(AuthOutcome::Restore(capability));
    }

    async fn try_restore_session(&self) -> Option<AuthenticatedSession> {
        self.set_activity(LoginActivity::LoadingSession);
        let session = self.load_saved_session().await?;
        let account = AccountScope::from_session(&session);

        self.set_activity(LoginActivity::OpeningStore);
        let passphrase = self.stored_passphrase(&account).await?;

        match self.restore_matrix_session(&session, &passphrase).await {
            Ok(capability) => Some(capability),
            Err(e) => {
                tracing::warn!("session restore failed, preserving local data: {e}");
                self.emit_show_login();
                self.emit_login_error(UserMessageKind::SessionRestoreFailed);
                None
            }
        }
    }

    async fn stored_passphrase(&self, account: &AccountScope) -> Option<String> {
        match self.storage.load_passphrase(account).await {
            Ok(Some(passphrase)) => Some(passphrase),
            Ok(None) => {
                tracing::warn!("no store key for the saved session, preserving local data");
                self.emit_show_login();
                self.fail_login_once(UserMessage::new(UserMessageKind::StoreKeyMissing));
                None
            }
            Err(e) => {
                tracing::warn!("failed to read the store key: {e}");
                self.emit_show_login();
                self.emit_login_error(UserMessageKind::StoreKeyUnreadable);
                None
            }
        }
    }

    async fn check_server(&self, homeserver: &str, attempt: u64) {
        tracing::info!(homeserver, "checking server");

        let passphrase = generate_passphrase();
        self.stash_pending_passphrase(passphrase.clone());

        self.discover_server(homeserver, passphrase.as_str(), attempt)
            .await;
    }

    fn stash_pending_passphrase(&self, passphrase: String) {
        if let Ok(mut guard) = self.pending_passphrase.lock() {
            *guard = Some(passphrase);
        }
    }

    fn pending_passphrase(&self) -> Option<String> {
        self.pending_passphrase.lock().ok()?.clone()
    }

    fn forget_pending_passphrase(&self) {
        if let Ok(mut guard) = self.pending_passphrase.lock() {
            *guard = None;
        }
    }

    async fn discover_server(&self, homeserver: &str, passphrase: &str, attempt: u64) {
        match self.auth.discover_auth(homeserver, passphrase).await {
            Ok(info) if info.auth_methods.is_empty() => {
                let flows = info.unsupported_flows.join(", ");
                tracing::warn!(homeserver, flows = %flows, "no supported login method");
                self.reject_auth(
                    attempt,
                    UserMessage::about(UserMessageKind::UnsupportedLoginMethod, &flows),
                );
            }
            Ok(info) => {
                if self.lifecycle.settle_auth(attempt) {
                    self.emit_server_info(info);
                }
            }
            Err(e) => {
                tracing::warn!(homeserver, "server discovery failed: {e}");
                self.fail_auth(attempt, UserMessageKind::ServerUnreachable);
            }
        }
    }

    async fn load_saved_session(&self) -> Option<Session> {
        match self.storage.load_session().await {
            Ok(StoredSession::Present(session)) => {
                tracing::info!(user_id = %session.user_id, "found saved session");
                Some(session)
            }
            unusable => {
                self.report_unusable_session(unusable);
                None
            }
        }
    }

    fn report_unusable_session(&self, loaded: Result<StoredSession>) {
        let (reason, error) = classify_unusable_session(loaded);
        if let Some(e) = &error {
            tracing::warn!("{reason}, preserving local data: {e}");
        } else {
            tracing::info!("{reason}, showing login");
        }

        self.emit_show_login();
        if error.is_some() {
            self.emit_login_error(UserMessageKind::SessionUnreadable);
        }
    }

    fn fail_auth(&self, attempt: u64, kind: UserMessageKind) {
        self.reject_auth(attempt, UserMessage::new(kind));
    }

    fn reject_auth(&self, attempt: u64, message: UserMessage) {
        if self.lifecycle.settle_auth(attempt) {
            self.fail_login_once(message);
        } else {
            tracing::debug!("auth failure for superseded attempt, dropping");
        }
    }

    async fn restore_matrix_session(
        &self,
        session: &Session,
        passphrase: &str,
    ) -> Result<AuthenticatedSession> {
        let output = Arc::clone(&self.output);
        let on_progress = Box::new(move |step| {
            let activity = restore_activity(step);
            output.publish(Box::new(move |view| view.lifecycle.activity = activity));
        });

        self.auth
            .restore_session(session, passphrase, on_progress)
            .await
    }

    async fn login_password(&self, creds: LoginCredentials, attempt: u64) {
        let outcome = match self.auth.login_password(creds).await {
            Ok(session) => self.establish_session(session).await,
            Err(e) => Err(e),
        };
        match outcome {
            Ok(established) => self.send_auth(AuthOutcome::Login {
                attempt,
                established,
            }),
            Err(e) => {
                tracing::warn!("password login failed: {e}");
                self.fail_auth(attempt, login_failure(&e));
            }
        }
    }

    async fn establish_session(&self, session: Session) -> Result<EstablishedSession> {
        let passphrase = self.pending_passphrase().ok_or_else(|| {
            AppError::Other("No login store was prepared. Please start again.".into())
        })?;

        let account = AccountScope::from_session(&session);
        let adoption = self.auth.adopt_session(&session, &passphrase).await?;
        let established = EstablishedSession::record_or_roll_back(
            adoption,
            Arc::clone(&self.storage),
            account,
            &session,
            &passphrase,
        )
        .await?;

        self.forget_pending_passphrase();
        Ok(established)
    }

    async fn login_oauth(&self, cancel: CancellationToken, attempt: u64) {
        let result = self.run_oauth_flow(&cancel).await;
        self.end_oauth().await;
        match result {
            Ok(Some(established)) => self.send_auth(AuthOutcome::Login {
                attempt,
                established,
            }),
            Ok(None) => {
                tracing::info!("OAuth login cancelled");
                self.set_activity(LoginActivity::Idle);
            }
            Err(e) => {
                tracing::warn!("OAuth login failed: {e}");
                self.fail_auth(attempt, login_failure(&e));
            }
        }
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
        let mut messages = vec![UserMessage::new(UserMessageKind::SessionExpired)];
        messages.extend(cleanup_problem(&report));
        self.fail_login(messages);
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
        self.emit_cleanup_problem(&report);
    }

    async fn destroy_store_key(&self, account: &AccountScope, report: &mut CleanupReport) {
        if let Err(e) = self.storage.clear_passphrase(account).await {
            report.fail(format!("the local store key could not be destroyed ({e})"));
        }
    }

    async fn clear_credentials(&self, report: &mut CleanupReport) {
        if let Err(e) = self.storage.clear_session().await {
            report.fail(e.to_string());
        }
    }

    fn emit_cleanup_problem(&self, report: &CleanupReport) {
        if let Some(problem) = cleanup_problem(report) {
            self.fail_login_once(problem);
        }
    }

    pub(super) fn spawn_session_persister(
        &self,
        group: &mut TaskGroup,
        lifecycle_port: Arc<dyn SessionPort>,
    ) {
        let storage = Arc::clone(&self.storage);
        let output = Arc::clone(&self.output);
        let token = group.token();
        group.spawn(async move {
            let (session_tx, mut session_rx) = mpsc::unbounded_channel::<Session>();
            let subscribe = lifecycle_port.subscribe_session_changes(session_tx);
            let persist = async {
                while let Some(session) = session_rx.recv().await {
                    if let Err(e) = storage.save_session(&session).await {
                        tracing::warn!("failed to persist refreshed session: {e}");
                        super::show_toast(
                            output.as_ref(),
                            Toast::Error(UserMessage::new(UserMessageKind::SessionSaveFailed)),
                        );
                    } else {
                        tracing::info!("persisted refreshed session tokens");
                    }
                }
            };

            tokio::select! {
                result = subscribe => {
                    if let Err(e) = result {
                        tracing::warn!("session change listener ended: {e}");
                    }
                }
                () = persist => {
                    tracing::debug!("session change persister stopped");
                }
                () = token.cancelled() => {
                    tracing::debug!("session change listener cancelled");
                }
            }
        });
    }

    pub(super) fn spawn_user_avatar_fetch(
        &self,
        group: &mut TaskGroup,
        lifecycle_port: Arc<dyn SessionPort>,
    ) {
        let output = Arc::clone(&self.output);
        group.spawn(async move {
            match lifecycle_port.fetch_user_avatar().await {
                Ok(path) => {
                    output.publish(Box::new(move |view| view.lifecycle.avatar_path = path));
                }
                Err(e) => tracing::debug!("user avatar fetch failed: {e}"),
            }
        });
    }

    async fn run_oauth_flow(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Option<EstablishedSession>> {
        let Some(session) = self.cancellable_browser_sign_in(cancel).await? else {
            return Ok(None);
        };
        self.set_activity(LoginActivity::OpeningStore);
        self.establish_session(session).await.map(Some)
    }

    async fn cancellable_browser_sign_in(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Option<Session>> {
        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(None),
            result = self.browser_sign_in() => result.map(Some),
        }
    }

    async fn browser_sign_in(&self) -> Result<Session> {
        let oauth_data = self.auth.login_oauth_start().await?;
        self.browser.open_url(&oauth_data.auth_url).await?;
        self.set_activity(LoginActivity::WaitingAuth);
        timeout(OAUTH_CALLBACK_TIMEOUT, self.auth.login_oauth_finish())
            .await
            .map_err(|_| AppError::Other("Timed out waiting for browser sign-in.".into()))?
    }

    fn begin_oauth(&self) -> CancellationToken {
        let token = CancellationToken::new();
        if let Ok(mut guard) = self.oauth_cancel.lock() {
            *guard = Some(token.clone());
        }
        token
    }

    async fn end_oauth(&self) {
        if let Ok(mut guard) = self.oauth_cancel.lock() {
            *guard = None;
        }
        self.auth.cancel_oauth().await;
    }

    async fn clear_local_state(
        &self,
        session: u64,
        account: &AccountScope,
        lifecycle_port: &dyn SessionPort,
    ) -> CleanupReport {
        if !self.lifecycle.begin_cleanup(session) {
            tracing::debug!("cleanup requested for a superseded session, skipping");
            return CleanupReport::default();
        }
        self.set_activity(LoginActivity::CleaningUp);

        let mut report = CleanupReport::default();
        self.destroy_store_key(account, &mut report).await;
        self.clear_credentials(&mut report).await;
        report.merge(lifecycle_port.clear_store().await);

        self.lifecycle.finish_logout(session);
        self.set_activity(LoginActivity::Idle);
        report
    }

    fn send_auth(&self, outcome: AuthOutcome) {
        if self.auth_tx.send(outcome).is_err() {
            tracing::debug!("auth outcome receiver gone; dropping authenticated session");
        }
    }

    fn emit_server_info(&self, info: ServerInfo) {
        let method = LoginMethod::from_auth_methods(&info.auth_methods);
        self.output.publish(Box::new(move |view| {
            view.lifecycle.method = method;
            view.lifecycle.resolved_homeserver = info.homeserver_url;
            view.lifecycle.step = LoginStep::Credentials;
            view.lifecycle.activity = LoginActivity::Idle;
        }));
    }

    fn emit_show_login(&self) {
        self.output.publish(Box::new(|view| {
            view.lifecycle.step = LoginStep::Homeserver;
            view.lifecycle.activity = LoginActivity::Idle;
        }));
    }

    fn emit_login_error(&self, kind: UserMessageKind) {
        self.fail_login_once(UserMessage::new(kind));
    }

    fn set_activity(&self, activity: LoginActivity) {
        self.output
            .publish(Box::new(move |view| view.lifecycle.activity = activity));
    }

    fn begin_login(&self, activity: LoginActivity) {
        self.output.publish(Box::new(move |view| {
            view.lifecycle.activity = activity;
            view.lifecycle.messages.clear();
        }));
    }

    fn fail_login(&self, messages: Vec<UserMessage>) {
        self.output.publish(Box::new(move |view| {
            view.lifecycle.activity = LoginActivity::Idle;
            view.lifecycle.messages = messages;
        }));
    }

    fn fail_login_once(&self, message: UserMessage) {
        self.fail_login(vec![message]);
    }
}
