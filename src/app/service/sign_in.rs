use std::path::PathBuf;

use super::{AppService, HeldSession, Reauth};
use crate::app::establish::{EstablishedSession, Rollback};
use crate::app::event::{EndReason, SessionEvent};
use crate::app::lifecycle::Settled;
use crate::app::{session, show_toast};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{LoginActivity, LoginStep, Toast};
use crate::domain::auth::ServerInfo;
use crate::ports::matrix::{AuthenticatedSession, CleanupReport};

async fn undo_superseded_login(established: EstablishedSession) -> Rollback {
    tracing::info!("authentication superseded, undoing the login");
    let rollback = established.roll_back().await;
    if let Rollback::Complete { report, .. } = &rollback {
        warn_if_not_undone(report);
    }
    rollback
}

fn warn_if_not_undone(report: &CleanupReport) {
    if !report.is_clean() {
        tracing::warn!("superseded login not fully undone: {}", report.summary());
    }
}

impl AppService {
    pub(super) fn block_sign_in(&mut self, message: UserMessage) {
        tracing::error!(
            "signing in is blocked until this is resolved: {detail}",
            detail = message.detail
        );
        self.lifecycle.block();
        self.blocked = Some(message);
        self.report_blocked();
    }

    pub(super) fn report_blocked(&self) {
        let Some(message) = self.blocked.clone() else {
            return;
        };
        self.output.publish(Box::new(move |view| {
            view.lifecycle.step = LoginStep::Homeserver;
            view.lifecycle.activity = LoginActivity::Idle;
            view.lifecycle.messages = vec![message];
        }));
    }

    pub(super) fn cancel_oauth(&mut self) {
        if self.lifecycle.cancel_auth() {
            self.session.cancel_oauth();
        }
    }

    pub(super) fn reauthenticate(&mut self, method: Reauth) {
        let Some(prior) = self
            .held_session
            .suspended()
            .map(|suspended| suspended.session.clone())
        else {
            return;
        };
        let Some(attempt) = self.lifecycle.begin_reauth() else {
            return;
        };
        match method {
            Reauth::Password(password) => {
                self.session
                    .spawn_reauth_password(&mut self.operations, prior, password, attempt);
            }
            Reauth::Browser => {
                self.session
                    .spawn_reauth_oauth(&mut self.operations, prior, attempt);
            }
        }
    }

    fn emit_login_success(&self, user_id: String, device_id: String) {
        self.output.publish(Box::new(move |view| {
            view.lifecycle.user_id = user_id;
            view.lifecycle.device_id = device_id;
            view.lifecycle.step = LoginStep::LoggedIn;
            view.lifecycle.activity = LoginActivity::Idle;
        }));
    }

    pub(super) async fn handle_session_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::RestoreProgress(activity) => self.settle_restore_progress(activity),
            SessionEvent::RestoreFailed(message) => self.settle_restore_failure(message),
            SessionEvent::RestorePaused(message) => self.settle_restore_pause(message),
            SessionEvent::Restored(capability) => self.settle_restore(*capability).await,
            SessionEvent::ServerDiscovered { attempt, info } => {
                self.settle_discovery(attempt, *info);
            }
            SessionEvent::AuthActivity { attempt, activity } => {
                self.settle_auth_activity(attempt, activity);
            }
            SessionEvent::AuthRejected { attempt, messages } => {
                self.settle_auth_rejection(attempt, messages);
            }
            SessionEvent::AuthCancelled { attempt } => self.settle_auth_cancel(attempt),
            SessionEvent::LoggedIn {
                attempt,
                established,
            } => self.settle_login(attempt, *established).await,
            SessionEvent::LoginUnresolved(message) => self.block_sign_in(message),
            SessionEvent::ErasingLocalState { session } => self.settle_erasure_start(session),
            SessionEvent::LocalStateCleared {
                session,
                reason,
                left_behind,
                report,
            } => self.settle_erasure(session, reason, left_behind.as_deref(), &report),
            SessionEvent::TokensNotPersisted { session } => {
                self.settle_tokens_not_persisted(session);
            }
            SessionEvent::UserAvatar { session, path } => self.settle_user_avatar(session, path),
            SessionEvent::Resumed {
                attempt,
                capability,
            } => self.settle_resume(attempt, *capability).await,
            SessionEvent::Suspended(reason) => self.suspend_session(reason).await,
            SessionEvent::Expired => self.end_session(EndReason::Expired).await,
        }
    }

    fn settle_restore_progress(&self, activity: LoginActivity) {
        if self.lifecycle.is_restoring() {
            self.session.set_activity(activity);
        }
    }

    fn settle_restore_failure(&mut self, messages: Vec<UserMessage>) {
        if self.lifecycle.restore_failed() {
            self.session.show_login(messages);
        } else {
            tracing::debug!("restore failure for a superseded restore, dropping");
        }
    }

    fn settle_restore_pause(&mut self, message: UserMessage) {
        if self.lifecycle.pause_restore() {
            self.session.show_restore_paused(message);
        } else {
            tracing::debug!("restore pause for a superseded restore, dropping");
        }
    }

    async fn settle_restore(&mut self, capability: AuthenticatedSession) {
        let Some(session) = self.lifecycle.restore_succeeded() else {
            tracing::info!("restore superseded, dropping session");
            return;
        };
        self.activate(capability, session).await;
    }

    fn settle_discovery(&mut self, attempt: u64, info: ServerInfo) {
        match self.lifecycle.settle_auth(attempt) {
            Some(Settled::Awaited) => self.session.show_credentials(info),
            Some(Settled::Cancelled) => self.session.set_activity(LoginActivity::Idle),
            None => tracing::debug!("server info for a superseded attempt, dropping"),
        }
    }

    fn settle_auth_activity(&self, attempt: u64, activity: LoginActivity) {
        if self.lifecycle.awaits(attempt) {
            self.session.set_activity(activity);
        } else {
            tracing::debug!("activity update for a cancelled or superseded attempt, dropping");
        }
    }

    fn settle_auth_rejection(&mut self, attempt: u64, messages: Vec<UserMessage>) {
        self.session.finish_oauth();
        match self.lifecycle.settle_auth(attempt) {
            Some(Settled::Awaited) => self.session.fail_login(messages),
            Some(Settled::Cancelled) => self.session.set_activity(LoginActivity::Idle),
            None => tracing::debug!("auth failure for a superseded attempt, dropping"),
        }
    }

    fn settle_auth_cancel(&mut self, attempt: u64) {
        self.session.finish_oauth();
        self.return_to_idle(attempt);
    }

    fn return_to_idle(&mut self, attempt: u64) {
        if self.lifecycle.settle_auth(attempt).is_some() {
            self.session.set_activity(LoginActivity::Idle);
        }
    }

    fn settle_undone_login(&mut self, attempt: u64, left_behind: Option<&str>) {
        let Some(device_id) = left_behind else {
            self.return_to_idle(attempt);
            return;
        };
        if self.lifecycle.settle_auth(attempt).is_some() {
            self.session
                .fail_login(vec![session::left_behind_notice(device_id)]);
        }
    }

    fn settle_tokens_not_persisted(&self, session: u64) {
        if self.lifecycle.runs(session) {
            show_toast(
                self.output.as_ref(),
                Toast::Error(UserMessage::new(UserMessageKind::SessionSaveFailed)),
            );
        } else {
            tracing::debug!("token save failure for a session that no longer runs, dropping");
        }
    }

    fn settle_user_avatar(&self, session: u64, path: Option<PathBuf>) {
        if self.lifecycle.runs(session) {
            self.output
                .publish(Box::new(move |view| view.lifecycle.avatar_path = path));
        } else {
            tracing::debug!("user avatar for a session that no longer runs, dropping");
        }
    }

    async fn settle_login(&mut self, attempt: u64, established: EstablishedSession) {
        self.session.finish_oauth();
        self.session.spend_pending_passphrase();
        let Some(session) = self.lifecycle.promote_to_syncing(attempt) else {
            match undo_superseded_login(established).await {
                Rollback::Complete { left_behind, .. } => {
                    self.settle_undone_login(attempt, left_behind.as_deref());
                }
                Rollback::Unresolved(message) => self.block_sign_in(message),
            }
            return;
        };
        self.activate(established.commit().await, session).await;
    }

    async fn settle_resume(&mut self, attempt: u64, capability: AuthenticatedSession) {
        self.session.finish_oauth();
        let Some(session) = self.lifecycle.resume_syncing(attempt) else {
            tracing::info!("re-authentication superseded, releasing the session it produced");
            capability.lifecycle.suspend().await;
            self.return_to_idle(attempt);
            return;
        };
        self.activate(capability, session).await;
    }

    async fn activate(&mut self, capability: AuthenticatedSession, session: u64) {
        let user_id = capability.session.user_id.clone();
        let device_id = capability.session.device_id.clone();
        tracing::info!(%user_id, "authenticated");
        self.held_session = HeldSession::Running(Box::new(capability));
        self.emit_login_success(user_id, device_id);
        self.start_syncing(session).await;
    }
}
