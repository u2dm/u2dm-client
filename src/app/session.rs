use std::fmt::Write;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::lifecycle::Lifecycle;
use super::task_group::TaskGroup;
use crate::commands::{Effect, LoginStep};
use crate::domain::models::{LoginCredentials, LoginMethod, ServerInfo, Session};
use crate::error::{AppError, Result};
use crate::ports::browser::BrowserPort;
use crate::ports::matrix::{AuthPort, AuthenticatedSession, SessionPort};
use crate::ports::output::AppOutputPort;
use crate::ports::storage::{StoragePort, StoredSession};

const OAUTH_CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

pub(super) enum AuthOutcome {
    Login {
        attempt: u64,
        session: AuthenticatedSession,
    },
    Restore(AuthenticatedSession),
}

#[allow(clippy::let_underscore_must_use)]
fn generate_passphrase() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
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
        let this = self.clone();
        group.spawn(async move { this.check_server(&homeserver, attempt).await });
    }

    pub(super) fn spawn_login_password(
        &self,
        group: &mut TaskGroup,
        creds: LoginCredentials,
        attempt: u64,
    ) {
        let this = self.clone();
        group.spawn(async move { this.login_password(creds, attempt).await });
    }

    pub(super) fn spawn_login_oauth(&self, group: &mut TaskGroup, attempt: u64) {
        let cancel = self.begin_oauth();
        let this = self.clone();
        group.spawn(async move { this.login_oauth(cancel, attempt).await });
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
        lifecycle_port: Arc<dyn SessionPort>,
    ) {
        let this = self.clone();
        group.spawn(async move { this.logout(session, lifecycle_port).await });
    }

    pub(super) fn spawn_expire_session(
        &self,
        group: &mut TaskGroup,
        session: u64,
        lifecycle_port: Arc<dyn SessionPort>,
    ) {
        let this = self.clone();
        group.spawn(async move { this.expire_session(session, lifecycle_port).await });
    }

    async fn restore_session(&self) {
        let Some(capability) = self.try_restore_session().await else {
            self.lifecycle.restore_failed();
            return;
        };
        self.send_auth(AuthOutcome::Restore(capability));
    }

    async fn try_restore_session(&self) -> Option<AuthenticatedSession> {
        self.output
            .emit_now(Effect::Status("loading-session".into()));
        let session = self.load_saved_session().await?;

        self.output.emit_now(Effect::Status("opening-store".into()));
        let passphrase = self.passphrase_or_login_error().await?;

        match self.restore_matrix_session(&session, &passphrase).await {
            Ok(capability) => Some(capability),
            Err(e) => {
                tracing::warn!("session restore failed, preserving local data: {e}");
                self.emit_show_login();
                self.emit_login_error(&e).await;
                None
            }
        }
    }

    async fn check_server(&self, homeserver: &str, attempt: u64) {
        tracing::info!(homeserver, "checking server");

        let Some(passphrase) = self.passphrase_or_discovery_error(attempt).await else {
            return;
        };

        self.discover_server(homeserver, passphrase.as_str(), attempt)
            .await;
    }

    async fn discover_server(&self, homeserver: &str, passphrase: &str, attempt: u64) {
        match self.auth.discover_auth(homeserver, passphrase).await {
            Ok(info) => {
                if self.lifecycle.settle_auth(attempt) {
                    self.emit_server_info(info);
                }
            }
            Err(e) => {
                tracing::warn!(homeserver, "server discovery failed: {e}");
                self.fail_auth(attempt, &e).await;
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
                self.report_unusable_session(unusable).await;
                None
            }
        }
    }

    async fn report_unusable_session(&self, loaded: Result<StoredSession>) {
        let (reason, error) = classify_unusable_session(loaded);
        if let Some(e) = &error {
            tracing::warn!("{reason}, preserving local data: {e}");
        } else {
            tracing::info!("{reason}, showing login");
        }

        self.emit_show_login();
        if let Some(e) = error {
            self.emit_login_error(&e).await;
        }
    }

    async fn passphrase_or_login_error(&self) -> Option<String> {
        match self.get_or_create_passphrase().await {
            Ok(passphrase) => Some(passphrase),
            Err(e) => {
                self.emit_show_login();
                self.emit_login_error(&e).await;
                None
            }
        }
    }

    async fn passphrase_or_discovery_error(&self, attempt: u64) -> Option<String> {
        match self.get_or_create_passphrase().await {
            Ok(passphrase) => Some(passphrase),
            Err(e) => {
                tracing::warn!("failed to get passphrase: {e}");
                self.fail_auth(attempt, &e).await;
                None
            }
        }
    }

    async fn fail_auth(&self, attempt: u64, err: &AppError) {
        if self.lifecycle.settle_auth(attempt) {
            self.emit_login_error(err).await;
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
        let on_progress = Box::new(move |msg| {
            output.emit_now(Effect::Status(msg));
        });

        self.auth
            .restore_session(session, passphrase, on_progress)
            .await
    }

    async fn login_password(&self, creds: LoginCredentials, attempt: u64) {
        match self.auth.login_password(creds).await {
            Ok(capability) => self.send_auth(AuthOutcome::Login {
                attempt,
                session: capability,
            }),
            Err(e) => {
                tracing::warn!("password login failed: {e}");
                self.fail_auth(attempt, &e).await;
            }
        }
    }

    async fn login_oauth(&self, cancel: CancellationToken, attempt: u64) {
        let result = self.run_oauth_flow(&cancel).await;
        self.end_oauth().await;
        match result {
            Ok(Some(capability)) => self.send_auth(AuthOutcome::Login {
                attempt,
                session: capability,
            }),
            Ok(None) => {
                tracing::info!("OAuth login cancelled");
                self.output.emit_now(Effect::Status(String::new()));
            }
            Err(e) => {
                tracing::warn!("OAuth login failed: {e}");
                self.fail_auth(attempt, &e).await;
            }
        }
    }

    async fn expire_session(&self, session: u64, lifecycle_port: Arc<dyn SessionPort>) {
        self.clear_local_state(lifecycle_port.as_ref()).await;
        self.lifecycle.finish_logout(session);
        self.output
            .emit(Effect::LoginError(
                "Session expired. Please log in again.".into(),
            ))
            .await;
    }

    async fn logout(&self, session: u64, lifecycle_port: Arc<dyn SessionPort>) {
        tracing::info!("user initiated logout");
        if let Err(e) = lifecycle_port.logout().await {
            tracing::warn!("failed to logout from server: {e}");
        }
        self.clear_local_state(lifecycle_port.as_ref()).await;
        self.lifecycle.finish_logout(session);
        tracing::info!("logout complete");
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
                        output
                            .emit(Effect::Toast(format!(
                                "Failed to save refreshed session: {e}"
                            )))
                            .await;
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
    ) -> Result<Option<AuthenticatedSession>> {
        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(None),
            result = self.oauth_login_steps() => result.map(Some),
        }
    }

    async fn oauth_login_steps(&self) -> Result<AuthenticatedSession> {
        let oauth_data = self.auth.login_oauth_start().await?;
        self.browser.open_url(&oauth_data.auth_url).await?;
        self.output.emit_now(Effect::Status("waiting-auth".into()));
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

    async fn get_or_create_passphrase(&self) -> Result<String> {
        if let Some(passphrase) = self.storage.load_passphrase().await? {
            return Ok(passphrase);
        }
        let passphrase = generate_passphrase();
        self.storage.save_passphrase(&passphrase).await?;
        Ok(passphrase)
    }

    pub(super) async fn save_session(&self, session: &Session) {
        if let Err(e) = self.storage.save_session(session).await {
            tracing::warn!("failed to save session: {e}");
            self.notify_error(format!(
                "Session not saved. You may need to log in again after restart: {e}"
            ))
            .await;
        }
    }

    async fn clear_credentials(&self) {
        if let Err(e) = self.storage.clear_session().await {
            tracing::warn!("failed to clear session: {e}");
        }
    }

    async fn clear_local_state(&self, lifecycle_port: &dyn SessionPort) {
        self.clear_credentials().await;
        if let Err(e) = lifecycle_port.clear_store().await {
            tracing::warn!("failed to clear store: {e}");
        }
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
        }));
        self.output.emit_now(Effect::Status(String::new()));
    }

    fn emit_show_login(&self) {
        self.output
            .publish(Box::new(|view| view.lifecycle.step = LoginStep::Homeserver));
        self.output.emit_now(Effect::Status(String::new()));
    }

    async fn emit_login_error(&self, err: &AppError) {
        self.output.emit(Effect::LoginError(err.to_string())).await;
    }

    async fn notify_error(&self, msg: impl Into<String> + Send) {
        self.output.emit(Effect::Toast(msg.into())).await;
    }
}
