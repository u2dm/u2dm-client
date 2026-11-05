use super::AppService;
use crate::app::account::{LogoutDecision, Settled as AccountSettled};
use crate::app::event::{AccountEvent, EncryptionEvent, EndReason, IdentityEvent};
use crate::app::identity_alerts::Resolution;
use crate::app::uia::Reply;
use crate::app::verification::VerificationAction;
use crate::commands::messages::UserMessageKind;
use crate::commands::ui::{AccountCommand, AuthCommand, KeysCommand};
use crate::domain::encryption::RecoveryKey;
use crate::domain::room::RoomId;
use crate::domain::verification::VerificationEvent;

impl AppService {
    pub(super) fn handle_identity(&mut self, event: IdentityEvent) {
        match event {
            IdentityEvent::RoomAlerts { watch, alerts } => {
                self.identity_alerts.room_alerts_changed(watch, alerts);
            }
            IdentityEvent::OwnIdentity { session, own } => {
                if self.lifecycle.runs(session) {
                    self.identity_alerts.own_identity_changed(own);
                } else {
                    tracing::debug!("own identity for a session that no longer runs, dropping");
                }
            }
            IdentityEvent::Settled { request, failure } => {
                if self.held_session.running().is_some() {
                    self.identity_alerts.settled(request, failure);
                }
            }
        }
    }

    pub(super) fn handle_encryption(&mut self, event: EncryptionEvent) {
        match event {
            EncryptionEvent::Changed { session, state } => {
                if !self.lifecycle.runs(session) {
                    tracing::debug!("encryption state of a session that no longer runs, dropping");
                    return;
                }
                self.encryption.state_changed(state);
            }
            EncryptionEvent::IdentitySettled { session, outcome } => {
                if !self.lifecycle.runs(session) {
                    tracing::debug!("identity setup of a session that no longer runs, dropping");
                    return;
                }
                self.encryption.identity_settled(outcome.as_ref());
            }
            EncryptionEvent::ActionSettled { request, outcome } => {
                let Some(port) = self.port(|a| &a.encryption) else {
                    return;
                };
                self.encryption.settled(Some(port), request, outcome);
            }
        }
        self.raise_session_alert();
        self.calls
            .trust_changed(self.encryption.is_verified(), &self.room_directory);
        self.user_info
            .session_trust_changed(self.encryption.is_verified());
    }

    pub(super) fn use_recovery_key(&mut self, key: RecoveryKey) {
        if let Some(port) = self.port(|a| &a.encryption) {
            self.encryption.use_key(port, key);
        }
    }

    pub(super) fn verify_with_other_session(&mut self) {
        if let Some(port) = self.port(|a| &a.verification) {
            self.encryption.verify_with_other_session(port);
        }
    }

    pub(super) fn set_up_recovery(&mut self) {
        if let Some(port) = self.port(|a| &a.encryption) {
            self.encryption.set_up_recovery(port);
        }
    }

    pub(super) fn change_recovery_key(&mut self) {
        if let Some(port) = self.port(|a| &a.encryption) {
            self.encryption.change_key(port);
        }
    }

    pub(super) fn retry_encryption_setup(&mut self) {
        if let Some(port) = self.port(|a| &a.encryption) {
            self.encryption.retry_setup(port);
            self.raise_session_alert();
        }
    }

    pub(super) fn handle_key_command(&mut self, command: KeysCommand) {
        let Some(port) = self.port(|a| &a.encryption) else {
            return;
        };
        match command {
            KeysCommand::StartIdentityReset => self.encryption.start_reset(),
            KeysCommand::ResetIdentity => self.encryption.reset_identity(port),
            KeysCommand::StartKeyExport => self.encryption.start_key_export(),
            KeysCommand::ExportRoomKeys(passphrase) => {
                self.encryption.export_keys(port, passphrase);
            }
            KeysCommand::StartKeyImport => self.encryption.start_key_import(),
            KeysCommand::ImportRoomKeys(passphrase) => {
                self.encryption.import_keys(port, passphrase);
            }
        }
    }

    pub(super) fn handle_account_command(&mut self, command: AccountCommand) {
        let Some(port) = self.port(|a| &a.account) else {
            return;
        };
        match command {
            AccountCommand::OpenAccount => self.account.open(&port),
            AccountCommand::CloseAccount => self.account.close(),
            AccountCommand::ShowAccountOverview => self.account.show_overview(),
            AccountCommand::RenameSession { device_id, name } => {
                self.account.rename_session(port, device_id, &name);
            }
            AccountCommand::SignOutSession(device_id) => {
                self.account.sign_out_session(port, device_id);
            }
            AccountCommand::OpenAccountPage(page) => self.account.open_account_page(port, page),
            AccountCommand::StartPasswordChange => self.account.start_password_change(),
            AccountCommand::ChangePassword {
                password,
                sign_out_others,
            } => self
                .account
                .change_password(port, password, sign_out_others),
            AccountCommand::StartDeactivation => self.account.start_deactivation(port),
            AccountCommand::DeactivateAccount { erase } => self.account.deactivate(port, erase),
        }
    }

    fn answer_auth(&mut self, reply: Reply) {
        if self.account.awaits_auth() {
            if let Some(port) = self.port(|a| &a.account) {
                self.account.answer_auth(port, reply);
            }
        } else if self.encryption.awaits_auth()
            && let Some(port) = self.port(|a| &a.encryption)
        {
            self.encryption.answer_auth(port, reply);
        }
    }

    fn reopen_auth_page(&mut self) {
        if self.account.awaits_auth() {
            self.account.reopen_auth_page();
        } else {
            self.encryption.reopen_auth_page();
        }
    }

    fn cancel_auth(&mut self) {
        if self.account.awaits_auth() {
            self.account.cancel_auth();
        } else if let Some(port) = self.port(|a| &a.encryption) {
            self.encryption.cancel_auth(port);
        }
    }

    pub(super) fn handle_auth_command(&mut self, command: AuthCommand) {
        match command {
            AuthCommand::AnswerAuthPassword(password) => {
                self.answer_auth(Reply::Password(password));
            }
            AuthCommand::ContinueAuth => self.answer_auth(Reply::Continue),
            AuthCommand::ReopenAuthPage => self.reopen_auth_page(),
            AuthCommand::CancelAuth => self.cancel_auth(),
        }
    }

    pub(super) async fn handle_account(&mut self, event: AccountEvent) {
        if self.held_session.running().is_none() {
            return;
        }
        match event {
            AccountEvent::FactsRead { generation, facts } => {
                self.account.facts_read(generation, facts);
            }
            AccountEvent::SessionsChanged {
                generation,
                sessions,
            } => self.account.sessions_changed(generation, sessions),
            AccountEvent::Settled { request, outcome } => {
                let port = self.port(|a| &a.account);
                if let AccountSettled::Deactivated = self.account.settled(port, request, outcome) {
                    tracing::info!("the account was deactivated, clearing local state");
                    self.end_session(EndReason::Deactivated).await;
                }
            }
            AccountEvent::LogoutChecked { request, risks } => {
                if let LogoutDecision::Proceed = self.account.logout_checked(request, risks) {
                    self.end_session(EndReason::UserLogout).await;
                }
            }
        }
    }

    fn raise_session_alert(&mut self) {
        self.identity_alerts
            .session_alert_changed(self.encryption.alert());
    }

    pub(super) fn resolve_identity_alert(&mut self, resolution: Resolution) {
        if let Some(port) = self.port(|a| &a.identity) {
            self.identity_alerts.resolve(port, resolution);
        }
    }

    pub(super) fn follow_identity_alerts(&mut self, room_id: RoomId) {
        if let Some(port) = self.port(|a| &a.identity) {
            self.identity_alerts.follow(port, room_id);
        }
    }

    pub(super) async fn settle_verification_flow(
        &mut self,
        session: u64,
        event: VerificationEvent,
    ) {
        if self.lifecycle.runs(session) {
            self.verification.flow_advanced(event).await;
        } else {
            tracing::debug!("verification update for a session that no longer runs, dropping");
        }
    }

    pub(super) async fn settle_verification_failure(&self, session: u64, failure: UserMessageKind) {
        if self.lifecycle.runs(session) {
            self.verification.action_failed(failure).await;
        } else {
            tracing::debug!("verification failure for a session that no longer runs, dropping");
        }
    }

    pub(super) async fn act_on_verification(&mut self, action: VerificationAction) {
        if let Some(verification) = self.port(|a| &a.verification) {
            self.verification
                .act(&mut self.operations, verification, action)
                .await;
        }
    }

    pub(super) async fn dismiss_verification(&mut self) {
        let verification = self.port(|a| &a.verification);
        self.verification
            .dismiss(&mut self.operations, verification)
            .await;
    }
}
