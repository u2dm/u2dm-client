use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::{EndingSession, left_behind_notice, send, unconfirmed_device};
use crate::app::establish::{EstablishedSession, Establishing, LoginFailure};
use crate::app::event::{EndReason, SessionEvent};
use crate::app::input::EventSender;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::LoginActivity;
use crate::domain::account::AccountScope;
use crate::domain::auth::{LoginCredentials, ServerInfo, Session};
use crate::domain::link::LauncherSafeUrl;
use crate::domain::sync::SyncSupport;
use crate::error::{AppError, AuthFailure, Result};
use crate::ports::browser::BrowserPort;
use crate::ports::matrix::{
    AuthPort, AuthenticatedSession, CleanupReport, DeviceRemoval, RestoreOutcome, RestoreStep,
    SessionPort,
};
use crate::ports::storage::{StoragePort, StoredSession};

const OAUTH_CALLBACK_TIMEOUT: Duration = Duration::from_mins(5);

fn unusable_server(homeserver: &str, info: &ServerInfo) -> Option<UserMessage> {
    if info.sync == SyncSupport::ClassicOnly {
        tracing::warn!(homeserver, "the homeserver does not offer sliding sync");
        return Some(UserMessage::new(UserMessageKind::SlidingSyncUnsupported));
    }
    if info.auth_methods.is_empty() {
        let flows = info.unsupported_flows.join(", ");
        tracing::warn!(homeserver, flows = %flows, "no supported login method");
        return Some(UserMessage::about(
            UserMessageKind::UnsupportedLoginMethod,
            &flows,
        ));
    }
    None
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
        AuthFailure::SessionNotReusable => UserMessageKind::ReauthNotPossible,
        AuthFailure::InsecureSignInPage => UserMessageKind::InsecureSignInPage,
        AuthFailure::Unknown => UserMessageKind::LoginFailed,
    }
}

fn keyring_unreadable(kind: UserMessageKind, cause: &AppError) -> UserMessage {
    if let AppError::Keyring { source, .. } = cause {
        return UserMessage::about(kind, source);
    }
    UserMessage::about(kind, cause)
}

fn restore_activity(step: RestoreStep) -> LoginActivity {
    match step {
        RestoreStep::Connecting => LoginActivity::Connecting,
        RestoreStep::RestoringAuth => LoginActivity::RestoringAuth,
    }
}

async fn end_server_session(
    lifecycle_port: &dyn SessionPort,
    quit: &CancellationToken,
) -> DeviceRemoval {
    tokio::select! {
        biased;
        () = quit.cancelled() => {
            tracing::warn!("quitting before the homeserver answered the logout, erasing local state anyway");
            DeviceRemoval::Unconfirmed
        }
        removal = lifecycle_port.logout() => removal,
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
pub(super) struct SessionTasks {
    pub(super) auth: Arc<dyn AuthPort>,
    pub(super) storage: Arc<dyn StoragePort>,
    pub(super) browser: Arc<dyn BrowserPort>,
    pub(super) events: EventSender,
}

impl SessionTasks {
    fn send(&self, event: SessionEvent) {
        send(&self.events, event);
    }

    pub(super) async fn restore_session(&self) {
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
            Ok(RestoreOutcome::Restored(capability)) => Some(*capability),
            Ok(RestoreOutcome::Diverged(removal)) => self.report_diverged(&session, removal).await,
            Err(e) => {
                tracing::warn!("session restore failed, preserving local data: {e}");
                self.report_restore_failed(vec![UserMessage::new(
                    UserMessageKind::SessionRestoreFailed,
                )])
            }
        }
    }

    fn report_restore_failed<T>(&self, messages: Vec<UserMessage>) -> Option<T> {
        self.send(SessionEvent::RestoreFailed(messages));
        None
    }

    async fn report_diverged<T>(&self, session: &Session, removal: DeviceRemoval) -> Option<T> {
        let mut messages = vec![UserMessage::new(UserMessageKind::IdentityDiverged)];
        match removal {
            DeviceRemoval::Confirmed => self.forget_signed_out_session().await,
            DeviceRemoval::Unconfirmed => messages.push(left_behind_notice(&session.device_id)),
        }
        self.report_restore_failed(messages)
    }

    async fn forget_signed_out_session(&self) {
        match self.storage.clear_session().await {
            Ok(()) => tracing::info!(
                "forgot the session the homeserver signed out, keeping its store and store key"
            ),
            Err(e) => tracing::warn!(
                "the session the homeserver signed out is still saved, so the next start restores a dead session: {e}"
            ),
        }
    }

    fn report_restore_paused<T>(&self, kind: UserMessageKind, cause: &AppError) -> Option<T> {
        self.send(SessionEvent::RestorePaused(keyring_unreadable(kind, cause)));
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
                    return self.report_restore_failed(Vec::new());
                };
                tracing::warn!("{reason}, preserving local data: {e}");
                self.report_restore_paused(UserMessageKind::SessionUnreadable, &e)
            }
        }
    }

    async fn stored_passphrase(&self, account: &AccountScope) -> Option<String> {
        match self.storage.load_passphrase(account).await {
            Ok(Some(passphrase)) => Some(passphrase),
            Ok(None) => {
                tracing::warn!("no store key for the saved session, preserving local data");
                self.report_restore_failed(vec![UserMessage::new(UserMessageKind::StoreKeyMissing)])
            }
            Err(e) => {
                tracing::warn!("failed to read the store key, preserving local data: {e}");
                self.report_restore_paused(UserMessageKind::StoreKeyUnreadable, &e)
            }
        }
    }

    async fn restore_matrix_session(
        &self,
        session: &Session,
        passphrase: &str,
    ) -> Result<RestoreOutcome> {
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

    pub(super) async fn check_server(&self, homeserver: &str, passphrase: &str, attempt: u64) {
        tracing::info!(homeserver, "checking server");
        match self.auth.discover_auth(homeserver, passphrase).await {
            Ok(info) => match unusable_server(homeserver, &info) {
                Some(refusal) => self.send(SessionEvent::AuthRejected {
                    attempt,
                    messages: vec![refusal],
                }),
                None => self.send(SessionEvent::ServerDiscovered {
                    attempt,
                    info: Box::new(info),
                }),
            },
            Err(e) => {
                tracing::warn!(homeserver, "server discovery failed: {e}");
                self.reject(attempt, UserMessageKind::ServerUnreachable);
            }
        }
    }

    pub(super) async fn login_password(
        &self,
        creds: LoginCredentials,
        passphrase: Option<String>,
        attempt: u64,
    ) {
        let outcome = match self.auth.login_password(creds).await {
            Ok(session) => self.establish_session(session, passphrase).await,
            Err(e) => Err(e.into()),
        };
        match outcome {
            Ok(established) => self.send(SessionEvent::LoggedIn {
                attempt,
                established: Box::new(established),
            }),
            Err(failure) => self.report_login_failure("password login", attempt, failure),
        }
    }

    pub(super) async fn login_oauth(
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
            Err(failure) => self.report_login_failure("OAuth login", attempt, failure),
        }
    }

    fn report_login_failure(&self, method: &str, attempt: u64, failure: LoginFailure) {
        match failure {
            LoginFailure::Rejected { error, left_behind } => {
                tracing::warn!("{method} failed: {error}");
                let mut messages = vec![UserMessage::new(login_failure(&error))];
                messages.extend(left_behind.as_deref().map(left_behind_notice));
                self.send(SessionEvent::AuthRejected { attempt, messages });
            }
            LoginFailure::Unresolved(message) => {
                self.send(SessionEvent::LoginUnresolved(message));
            }
        }
    }

    async fn run_oauth_flow(
        &self,
        cancel: &CancellationToken,
        passphrase: Option<String>,
        attempt: u64,
    ) -> Establishing<Option<EstablishedSession>> {
        let Some(session) = unless_cancelled(cancel, self.browser_sign_in(attempt)).await? else {
            return Ok(None);
        };
        self.send(SessionEvent::AuthActivity {
            attempt,
            activity: LoginActivity::OpeningStore,
        });
        self.establish_session(session, passphrase).await.map(Some)
    }

    async fn browser_sign_in(&self, attempt: u64) -> Result<Session> {
        let oauth_data = self.auth.login_oauth_start().await?;
        self.await_browser(
            attempt,
            &oauth_data.auth_url,
            self.auth.login_oauth_finish(),
        )
        .await
    }

    async fn await_browser<T: Send>(
        &self,
        attempt: u64,
        auth_url: &LauncherSafeUrl,
        finish: impl Future<Output = Result<T>> + Send,
    ) -> Result<T> {
        self.browser.open_url(auth_url).await?;
        self.send(SessionEvent::AuthActivity {
            attempt,
            activity: LoginActivity::WaitingAuth,
        });
        timeout(OAUTH_CALLBACK_TIMEOUT, finish)
            .await
            .map_err(|_| AppError::Other("Timed out waiting for browser sign-in.".into()))?
    }

    async fn establish_session(
        &self,
        session: Session,
        passphrase: Option<String>,
    ) -> Establishing<EstablishedSession> {
        let passphrase = passphrase.ok_or_else(|| {
            AppError::Other("No login store was prepared. Please start again.".into())
        })?;

        let account = AccountScope::from_session(&session);
        let adoption = self.auth.adopt_session(&session, &passphrase).await?;
        EstablishedSession::record_or_roll_back(
            adoption,
            Arc::clone(&self.auth),
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
            messages: vec![UserMessage::new(kind)],
        });
    }

    async fn preserved_passphrase(&self, account: &AccountScope, attempt: u64) -> Option<String> {
        match self.storage.load_passphrase(account).await {
            Ok(Some(passphrase)) => Some(passphrase),
            Ok(None) => {
                tracing::warn!("no store key for the suspended session, preserving local data");
                self.reject(attempt, UserMessageKind::StoreKeyMissing);
                None
            }
            Err(e) => {
                tracing::warn!("failed to read the store key: {e}");
                self.send(SessionEvent::AuthRejected {
                    attempt,
                    messages: vec![keyring_unreadable(UserMessageKind::StoreKeyUnreadable, &e)],
                });
                None
            }
        }
    }

    pub(super) async fn reauth_password(&self, prior: Session, password: String, attempt: u64) {
        let account = AccountScope::from_session(&prior);
        let Some(passphrase) = self.preserved_passphrase(&account, attempt).await else {
            return;
        };
        let creds = LoginCredentials {
            username: prior.user_id.clone(),
            password,
        };
        match self.auth.reauthenticate(&prior, &passphrase, creds).await {
            Ok(capability) => self.report_resumed(attempt, capability).await,
            Err(e) => {
                tracing::warn!("password re-authentication failed: {e}");
                self.reject(attempt, login_failure(&e));
            }
        }
    }

    pub(super) async fn reauth_oauth(
        &self,
        prior: Session,
        cancel: CancellationToken,
        attempt: u64,
    ) {
        let account = AccountScope::from_session(&prior);
        let Some(passphrase) = self.preserved_passphrase(&account, attempt).await else {
            return;
        };
        let result = unless_cancelled(
            &cancel,
            self.reauth_browser_sign_in(&prior, &passphrase, attempt),
        )
        .await;
        self.auth.cancel_oauth().await;
        match result {
            Ok(Some(capability)) => self.report_resumed(attempt, capability).await,
            Ok(None) => {
                tracing::info!("OAuth re-authentication cancelled");
                self.send(SessionEvent::AuthCancelled { attempt });
            }
            Err(e) => {
                tracing::warn!("OAuth re-authentication failed: {e}");
                self.reject(attempt, login_failure(&e));
            }
        }
    }

    async fn reauth_browser_sign_in(
        &self,
        prior: &Session,
        passphrase: &str,
        attempt: u64,
    ) -> Result<AuthenticatedSession> {
        let oauth_data = self.auth.reauth_oauth_start(prior, passphrase).await?;
        let finish = self.auth.reauth_oauth_finish(prior);
        self.await_browser(attempt, &oauth_data.auth_url, finish)
            .await
    }

    async fn report_resumed(&self, attempt: u64, capability: AuthenticatedSession) {
        if let Err(e) = self.storage.save_session(&capability.session).await {
            tracing::warn!("the re-authenticated session is not saved yet: {e}");
        }
        self.send(SessionEvent::Resumed {
            attempt,
            capability: Box::new(capability),
        });
    }

    pub(super) async fn erase_session(
        &self,
        session: u64,
        ending: &EndingSession,
        reason: EndReason,
        left_behind: Option<String>,
    ) {
        let report = self
            .clear_local_state(session, &ending.account, ending.lifecycle.as_ref())
            .await;
        self.send(SessionEvent::LocalStateCleared {
            session,
            reason,
            left_behind,
            report,
        });
    }

    pub(super) async fn logout(
        &self,
        session: u64,
        ending: &EndingSession,
        quit: &CancellationToken,
    ) {
        tracing::info!("user initiated logout");
        let removal = end_server_session(ending.lifecycle.as_ref(), quit).await;
        let left_behind = unconfirmed_device(removal, &ending.device_id);
        self.erase_session(session, ending, EndReason::UserLogout, left_behind)
            .await;
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

async fn unless_cancelled<T: Send>(
    cancel: &CancellationToken,
    work: impl Future<Output = Result<T>> + Send,
) -> Result<Option<T>> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Ok(None),
        result = work => result.map(Some),
    }
}
