use std::future::pending;
use std::mem;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use tokio::sync::mpsc;

use super::{DemoAuthed, OutgoingFlow, unavailable, unavailable_if};
use crate::adapters::demo::revisions::report_changes;
use crate::adapters::demo::{account, data, encryption, identity, user_info, verification};
use crate::domain::capabilities::{AccountFacts, AccountPage};
use crate::domain::encryption::{
    AccountEncryption, IdentityOutcome, KeyImport, KeyPassphrase, Recovery, RecoveryAttempt,
    RecoveryKey, RecoverySetup,
};
use crate::domain::identity::{OwnIdentity, RoomAlert};
use crate::domain::link::LauncherSafeUrl;
use crate::domain::member_name::NamedUser;
use crate::domain::room::RoomId;
use crate::domain::sessions::{LogoutRisks, OwnSessions};
use crate::domain::uia::{AccountPassword, AuthAnswer, Guarded};
use crate::domain::user_info::{IdentityTrust, UserId};
use crate::domain::verification::{VerificationCancellation, VerificationEvent};
use crate::error::{AppError, Result};
use crate::ports::matrix::{AccountPort, EncryptionPort, IdentityPort, VerificationPort};

impl DemoAuthed {
    fn verification_listener(&self) -> Result<mpsc::UnboundedSender<VerificationEvent>> {
        self.verification_tx
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .ok_or_else(|| AppError::Other("verification requests are not followed".to_owned()))
    }

    fn emit_verification(&self, event: VerificationEvent) {
        let Ok(guard) = self.verification_tx.lock() else {
            return;
        };
        let Some(tx) = guard.as_ref() else {
            return;
        };
        if tx.send(event).is_err() {
            tracing::debug!("demo: verification listener is gone");
        }
    }

    async fn stop_verification(&self, reason: VerificationCancellation) -> Result<()> {
        if self.take_outgoing() != OutgoingFlow::None {
            self.flows.fetch_add(1, Ordering::Relaxed);
            verification::pause().await;
            let reason = match reason {
                VerificationCancellation::Declined => VerificationCancellation::Withdrawn,
                reason => reason,
            };
            self.emit_verification(VerificationEvent::Cancelled(reason));
            return Ok(());
        }
        let Some(demo) = verification::requested() else {
            return Err(unavailable("Verification"));
        };
        verification::pause().await;
        if demo.fails == verification::FailingStep::Reject {
            return Err(unavailable("Declining a verification"));
        }
        self.emit_verification(VerificationEvent::Cancelled(reason));
        Ok(())
    }

    async fn confirm_own_request(&self) -> Result<()> {
        verification::pause().await;
        let for_this_session = self
            .outgoing
            .lock()
            .is_ok_and(|outgoing| *outgoing == OutgoingFlow::Session);
        if for_this_session && encryption::scenario().fails {
            return Err(unavailable("Confirming a verification"));
        }
        self.emit_verification(VerificationEvent::Confirming);
        verification::pause().await;
        match self.take_outgoing() {
            OutgoingFlow::None => return Ok(()),
            OutgoingFlow::Session => {
                self.encryption.verified_by_another_session();
                tracing::debug!("demo: another session verified this one");
            }
            OutgoingFlow::Person(user_id) => {
                self.verified_people.verify(&user_id);
                tracing::debug!(%user_id, "demo: verified the person");
            }
        }
        self.emit_verification(VerificationEvent::Done);
        Ok(())
    }

    fn take_outgoing(&self) -> OutgoingFlow {
        self.outgoing
            .lock()
            .map(|mut outgoing| mem::take(&mut *outgoing))
            .unwrap_or_default()
    }

    fn outgoing_runs(&self) -> bool {
        self.outgoing
            .lock()
            .is_ok_and(|outgoing| *outgoing != OutgoingFlow::None)
    }

    fn begin_outgoing(&self, flow: OutgoingFlow) -> u64 {
        if let Ok(mut outgoing) = self.outgoing.lock() {
            *outgoing = flow;
        }
        self.sas_started.store(false, Ordering::Relaxed);
        self.flows.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
    }
}

#[async_trait]
impl EncryptionPort for DemoAuthed {
    async fn follow(&self, state_tx: mpsc::Sender<AccountEncryption>) -> Result<()> {
        encryption::pause_first_assessment().await;
        report_changes(self.encryption.subscribe(), &state_tx, || {
            self.encryption.state()
        })
        .await;
        Ok(())
    }

    async fn set_up_identity(&self, auth: Option<&AuthAnswer>) -> Result<IdentityOutcome> {
        if self.encryption.has_identity() {
            return Ok(IdentityOutcome::Ready);
        }
        let attempt = self.encryption.count_setup_attempt();
        encryption::pause_setup().await;
        if encryption::identity_refused()
            && let Guarded::NeedsAuth(challenge) = self.account.guard(auth)
        {
            tracing::debug!(
                "demo: the homeserver wants a password for the first cross-signing keys"
            );
            return Ok(IdentityOutcome::NeedsAuthentication(challenge));
        }
        if encryption::first_setup_fails(attempt) {
            return Err(unavailable("Creating the cross-signing identity"));
        }
        tracing::debug!("demo: created the account's cross-signing identity");
        self.encryption.create_identity();
        Ok(IdentityOutcome::Ready)
    }

    async fn recover(&self, key: &RecoveryKey) -> Result<RecoveryAttempt> {
        encryption::pause_action().await;
        unavailable_if(encryption::scenario().fails, "Using a recovery key")?;
        if !encryption::opens(key) {
            tracing::debug!("demo: the recovery key does not open the secret storage");
            return Ok(RecoveryAttempt::WrongKey);
        }
        let attempt = encryption::unlock_attempt();
        self.encryption.unlocked(attempt);
        Ok(attempt)
    }

    async fn enable_recovery(&self) -> Result<RecoverySetup> {
        encryption::pause_action().await;
        unavailable_if(encryption::scenario().fails, "Setting up recovery")?;
        self.encryption.recovery_enabled();
        tracing::debug!("demo: set up recovery");
        Ok(RecoverySetup::Created(encryption::created_key()))
    }

    async fn change_recovery_key(&self) -> Result<RecoveryKey> {
        encryption::pause_action().await;
        unavailable_if(encryption::scenario().fails, "Changing the recovery key")?;
        tracing::debug!("demo: changed the recovery key");
        Ok(encryption::changed_key())
    }

    async fn reset_identity(&self) -> Result<Guarded> {
        encryption::pause_action().await;
        unavailable_if(encryption::scenario().fails, "Resetting the identity")?;
        tracing::debug!("demo: the homeserver asks who you are before the new identity");
        Ok(self.account.guard_identity_reset(None))
    }

    async fn finish_identity_reset(&self, auth: &AuthAnswer) -> Result<Guarded> {
        encryption::pause_action().await;
        let guarded = self.account.guard_identity_reset(Some(auth));
        if guarded == Guarded::Done {
            tracing::debug!("demo: published the account's new identity");
            self.encryption.identity_reset();
        }
        Ok(guarded)
    }

    async fn abandon_identity_reset(&self) {
        tracing::debug!("demo: abandoned the identity reset");
    }

    async fn export_room_keys(&self, path: &Path, _passphrase: &KeyPassphrase) -> Result<usize> {
        encryption::pause_action().await;
        unavailable_if(encryption::scenario().fails, "Exporting room keys")?;
        tracing::info!(path = %path.display(), "demo: exported the room keys without writing a file");
        Ok(encryption::EXPORTED_KEYS)
    }

    async fn import_room_keys(&self, path: &Path, passphrase: &KeyPassphrase) -> Result<KeyImport> {
        encryption::pause_action().await;
        unavailable_if(encryption::scenario().fails, "Importing room keys")?;
        tracing::info!(path = %path.display(), "demo: importing room keys");
        Ok(encryption::import_from(passphrase))
    }
}

#[async_trait]
impl AccountPort for DemoAuthed {
    async fn facts(&self) -> AccountFacts {
        account::pause().await;
        account::facts()
    }

    async fn follow_sessions(&self, sessions_tx: mpsc::Sender<OwnSessions>) -> Result<()> {
        let device_id = data::session().device_id;
        let mut revision = self.account.subscribe();
        loop {
            if sessions_tx
                .send(self.account.sessions(&device_id))
                .await
                .is_err()
            {
                return Ok(());
            }
            if revision.changed().await.is_err() {
                return Ok(());
            }
        }
    }

    async fn rename_session(&self, device_id: &str, name: &str) -> Result<()> {
        account::pause().await;
        unavailable_if(account::scenario().fails, "Renaming a session")?;
        tracing::debug!(%device_id, "demo: renamed the session");
        self.account
            .rename(&data::session().device_id, device_id, name);
        Ok(())
    }

    async fn sign_out_session(
        &self,
        device_id: &str,
        auth: Option<&AuthAnswer>,
    ) -> Result<Guarded> {
        account::pause().await;
        unavailable_if(account::scenario().fails, "Signing out a session")?;
        let guarded = self.account.guard(auth);
        if guarded == Guarded::Done {
            tracing::debug!(%device_id, "demo: signed the session out");
            self.account.remove(device_id);
        }
        Ok(guarded)
    }

    async fn change_password(
        &self,
        _password: &AccountPassword,
        sign_out_others: bool,
        auth: Option<&AuthAnswer>,
    ) -> Result<Guarded> {
        account::pause().await;
        unavailable_if(account::scenario().fails, "Changing the password")?;
        let guarded = self.account.guard(auth);
        if guarded != Guarded::Done {
            return Ok(guarded);
        }
        if account::scenario().weak_password {
            return Err(AppError::WeakPassword(
                account::WEAK_PASSWORD_REASON.to_owned(),
            ));
        }
        tracing::debug!(sign_out_others, "demo: changed the password");
        if sign_out_others {
            self.account.remove_all_others();
        }
        Ok(Guarded::Done)
    }

    async fn deactivate(&self, erase: bool, auth: Option<&AuthAnswer>) -> Result<Guarded> {
        account::pause().await;
        unavailable_if(account::scenario().fails, "Deactivating the account")?;
        let guarded = self.account.guard(auth);
        if guarded == Guarded::Done {
            tracing::info!(erase, "demo: deactivated the account");
        }
        Ok(guarded)
    }

    async fn page(&self, page: &AccountPage) -> Option<LauncherSafeUrl> {
        let url = account::page(page)?;
        if let AccountPage::SignOut(device_id) = page {
            let book = Arc::clone(&self.account);
            let device_id = device_id.clone();
            tokio::spawn(async move {
                account::signed_out_in_browser(&book, &device_id).await;
            });
        }
        Some(url)
    }

    async fn logout_risks(&self) -> LogoutRisks {
        self.account
            .logout_risks(self.encryption.state().recovery == Recovery::On)
    }
}

#[async_trait]
impl IdentityPort for DemoAuthed {
    async fn follow_room(
        &self,
        room_id: &RoomId,
        alerts_tx: mpsc::Sender<Vec<RoomAlert>>,
    ) -> Result<()> {
        if !data::room_is_encrypted(room_id) {
            return Ok(());
        }
        identity::pause_room_alerts().await;
        report_changes(self.identity.subscribe(), &alerts_tx, || {
            self.identity.room_alerts(room_id)
        })
        .await;
        Ok(())
    }

    async fn follow_own(&self, own_tx: mpsc::Sender<OwnIdentity>) -> Result<()> {
        identity::pause_own_identity().await;
        report_changes(self.identity.subscribe(), &own_tx, || {
            self.identity.own_identity()
        })
        .await;
        Ok(())
    }

    async fn pin_identity(&self, user_id: &UserId) -> Result<()> {
        identity::pause_action().await;
        unavailable_if(identity::scenario().acks_fail, "accepting a new identity")?;
        tracing::debug!(%user_id, "demo: accepting a new identity");
        self.identity.accept(user_id).map_err(AppError::Other)
    }

    async fn withdraw_verification(&self, user_id: &UserId) -> Result<()> {
        identity::pause_action().await;
        unavailable_if(identity::scenario().acks_fail, "withdrawing a verification")?;
        tracing::debug!(%user_id, "demo: withdrawing a verification");
        self.identity.withdraw(user_id).map_err(AppError::Other)
    }

    async fn acknowledge_own_reset(&self) -> Result<()> {
        identity::pause_action().await;
        if identity::scenario().acks_fail {
            return Err(unavailable("acknowledging an identity reset"));
        }
        tracing::debug!("demo: acknowledging the reset of your identity");
        self.identity.acknowledge_own_reset();
        Ok(())
    }
}

#[async_trait]
impl VerificationPort for DemoAuthed {
    async fn listen_for_verification(
        &self,
        tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Result<()> {
        if let Ok(mut guard) = self.verification_tx.lock() {
            *guard = Some(tx);
        }

        if let Some(demo) = verification::requested() {
            verification::wait_for_request().await;
            self.sas_started.store(false, Ordering::Relaxed);
            self.emit_verification(verification::request(demo));

            if demo.times_out {
                verification::wait_for_request().await;
                self.emit_verification(VerificationEvent::Cancelled(
                    VerificationCancellation::TimedOut,
                ));
            }
        }

        pending::<()>().await;
        Ok(())
    }

    async fn verify_this_session(&self) -> Result<()> {
        encryption::pause_action().await;
        unavailable_if(encryption::scenario().fails, "Asking your other sessions")?;
        if !self.encryption.state().verified_sessions {
            return Err(AppError::Other(
                "no other session of this account can verify this one".to_owned(),
            ));
        }
        let tx = self.verification_listener()?;
        let flow = self.begin_outgoing(OutgoingFlow::Session);
        tracing::debug!("demo: asked the account's other sessions to verify this one");
        self.emit_verification(VerificationEvent::RequestSent {
            to: data::own_user().to_owned(),
            is_self: true,
        });
        let flows = Arc::clone(&self.flows);
        let sas_started = Arc::clone(&self.sas_started);
        tokio::spawn(async move {
            encryption::pause_sas().await;
            if flows.load(Ordering::Relaxed) == flow {
                sas_started.store(true, Ordering::Relaxed);
                if tx.send(verification::emojis()).is_err() {
                    tracing::debug!("demo: verification listener is gone");
                }
            }
        });
        Ok(())
    }

    async fn verify_user(&self, user_id: &UserId) -> Result<()> {
        user_info::pause().await;
        if user_info::scenario().verify_fails {
            return Err(unavailable("Asking people to verify"));
        }
        let Some(chat) = self.chat_with(user_id) else {
            return Err(AppError::Other(
                "there is no direct chat with this person to send the request in".to_owned(),
            ));
        };
        if self.trust_of(user_id) != IdentityTrust::Unverified {
            return Err(AppError::Other(
                "this person cannot be verified now".to_owned(),
            ));
        }
        let tx = self.verification_listener()?;
        let flow = self.begin_outgoing(OutgoingFlow::Person(user_id.to_string()));
        tracing::debug!(%user_id, room_id = %chat, "demo: asked the person to verify in your chat");
        let target = NamedUser::new(user_id.to_string(), data::names_in(&chat, []).of(user_id));
        let request =
            data::own_verification_request(self.sent.fetch_add(1, Ordering::Relaxed), target);
        self.post_own_row(&chat, request).await;
        self.emit_verification(VerificationEvent::RequestSent {
            to: user_id.to_string(),
            is_self: false,
        });
        let flows = Arc::clone(&self.flows);
        let sas_started = Arc::clone(&self.sas_started);
        let outgoing = Arc::clone(&self.outgoing);
        let answer = user_info::scenario().verify_answer;
        tokio::spawn(async move {
            let event = match answer {
                user_info::VerifyAnswer::Emoji => {
                    encryption::pause_sas().await;
                    verification::emojis()
                }
                user_info::VerifyAnswer::Declined => {
                    encryption::pause_sas().await;
                    VerificationEvent::Cancelled(VerificationCancellation::Remote)
                }
                user_info::VerifyAnswer::Unanswered => {
                    verification::wait_for_request().await;
                    VerificationEvent::Cancelled(VerificationCancellation::TimedOut)
                }
            };
            if flows.load(Ordering::Relaxed) != flow {
                return;
            }
            let ended = matches!(event, VerificationEvent::Cancelled(_));
            if ended && let Ok(mut outgoing) = outgoing.lock() {
                *outgoing = OutgoingFlow::None;
            }
            sas_started.store(
                matches!(event, VerificationEvent::Emojis(_)),
                Ordering::Relaxed,
            );
            if tx.send(event).is_err() {
                tracing::debug!("demo: verification listener is gone");
            }
        });
        Ok(())
    }

    async fn accept_verification(&self) -> Result<()> {
        let Some(demo) = verification::requested() else {
            return Err(unavailable("Verification"));
        };
        verification::pause().await;
        if demo.fails == verification::FailingStep::Accept {
            return Err(unavailable("Accepting a verification"));
        }
        self.sas_started.store(true, Ordering::Relaxed);
        self.emit_verification(verification::emojis());
        Ok(())
    }

    async fn confirm_verification(&self) -> Result<()> {
        if self.outgoing_runs() {
            return self.confirm_own_request().await;
        }
        let Some(demo) = verification::requested() else {
            return Err(unavailable("Verification"));
        };
        verification::pause().await;
        if demo.fails == verification::FailingStep::Confirm {
            return Err(unavailable("Confirming a verification"));
        }
        self.emit_verification(VerificationEvent::Confirming);
        verification::pause().await;
        self.emit_verification(VerificationEvent::Done);
        Ok(())
    }

    async fn reject_verification(&self) -> Result<()> {
        let reason = if self.sas_started.load(Ordering::Relaxed) {
            VerificationCancellation::Mismatch
        } else {
            VerificationCancellation::Declined
        };
        self.stop_verification(reason).await
    }

    async fn cancel_verification(&self) -> Result<()> {
        self.stop_verification(VerificationCancellation::Declined)
            .await
    }
}
