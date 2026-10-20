use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;

use super::event::{AppEvent, EncryptionEvent};
use super::input::EventSender;
use super::show_toast;
use super::task_group::TaskGroup;
use super::uia::{AuthRound, Next, Reply};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{
    EncryptionActivity, EncryptionDialog, EncryptionStep, EncryptionView, Toast,
};
use crate::domain::encryption::{
    AccountEncryption, IdentityOutcome, IdentitySetup, KeyImport, KeyPassphrase, Recovery,
    RecoveryAttempt, RecoveryKey, RecoverySetup, Standing,
};
use crate::domain::identity::SessionAlert;
use crate::domain::link::LauncherSafeUrl;
use crate::domain::uia::{AuthAnswer, AuthChallenge, Guarded};
use crate::ports::browser::BrowserPort;
use crate::ports::matrix::{EncryptionPort, VerificationPort};
use crate::ports::media::MediaFilePort;
use crate::ports::output::AppOutputPort;

const STATE_CHANNEL_CAP: usize = 8;
const KEY_EXPORT_FILENAME: &str = "u2dm-room-keys.txt";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Request,
    Unlock,
    SetUp,
    Change,
    Retry,
    Reset,
    Authenticate,
    ChooseFile,
    Export,
    Import,
}

impl Action {
    fn activity(self) -> EncryptionActivity {
        match self {
            Self::Request => EncryptionActivity::Requesting,
            Self::Unlock => EncryptionActivity::Unlocking,
            Self::SetUp => EncryptionActivity::SettingUpRecovery,
            Self::Change => EncryptionActivity::ChangingKey,
            Self::Retry => EncryptionActivity::RetryingSetup,
            Self::Reset => EncryptionActivity::ResettingIdentity,
            Self::Authenticate => EncryptionActivity::Authenticating,
            Self::ChooseFile => EncryptionActivity::ChoosingFile,
            Self::Export => EncryptionActivity::ExportingKeys,
            Self::Import => EncryptionActivity::ImportingKeys,
        }
    }
}

pub(super) enum EncryptionOutcome {
    Requested,
    Tried(RecoveryAttempt),
    SetUp(RecoverySetup),
    Changed(RecoveryKey),
    IdentitySettled(Option<IdentityOutcome>),
    Reset(Option<Guarded>),
    FileChosen(KeyFile, Option<PathBuf>),
    Exported(Option<usize>),
    Imported(Option<KeyImport>),
    Failed(UserMessageKind),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum KeyFile {
    Export,
    Import,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Guard {
    SetUp,
    Reset,
}

enum Step {
    Shown(EncryptionStep),
    Authenticating { guard: Guard, round: Box<AuthRound> },
    Exporting(PathBuf),
    Importing(PathBuf),
}

struct Pending {
    request: u64,
    action: Action,
}

struct Dialog {
    step: Step,
    error: UserMessage,
}

impl Dialog {
    fn at(step: EncryptionStep) -> Self {
        Self {
            step: Step::Shown(step),
            error: UserMessage::default(),
        }
    }

    fn shows(&self, step: &EncryptionStep) -> bool {
        matches!(&self.step, Step::Shown(shown) if shown == step)
    }
}

pub(super) struct Encryption {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    files: Arc<dyn MediaFilePort>,
    browser: Arc<dyn BrowserPort>,
    actions: TaskGroup,
    requests: u64,
    first_request_of_session: u64,
    state: AccountEncryption,
    setup: IdentitySetup,
    dialog: Option<Dialog>,
    pending: Option<Pending>,
}

impl Encryption {
    pub(super) fn new(
        output: Arc<dyn AppOutputPort>,
        events: EventSender,
        files: Arc<dyn MediaFilePort>,
        browser: Arc<dyn BrowserPort>,
    ) -> Self {
        Self {
            output,
            events,
            files,
            browser,
            actions: TaskGroup::new("encryption-actions"),
            requests: 0,
            first_request_of_session: 1,
            state: AccountEncryption::default(),
            setup: IdentitySetup::Running,
            dialog: None,
            pending: None,
        }
    }

    pub(super) fn begin_session(
        &mut self,
        group: &mut TaskGroup,
        port: &Arc<dyn EncryptionPort>,
        session: u64,
    ) {
        self.state = AccountEncryption::default();
        self.setup = IdentitySetup::Running;
        self.publish();

        let follow_port = Arc::clone(port);
        let events = self.events.clone();
        let token = group.token();
        group.spawn(async move {
            let (state_tx, state_rx) = mpsc::channel(STATE_CHANNEL_CAP);
            let follow = async {
                tokio::join!(
                    follow(follow_port.as_ref(), state_tx),
                    forward_states(&events, session, state_rx),
                )
            };
            token.run_until_cancelled(follow).await;
        });

        let setup_port = Arc::clone(port);
        let events = self.events.clone();
        let token = group.token();
        group.spawn(async move {
            let Some(outcome) = token
                .run_until_cancelled(set_up(setup_port.as_ref(), None))
                .await
            else {
                return;
            };
            drop(
                events.send(AppEvent::Encryption(EncryptionEvent::IdentitySettled {
                    session,
                    outcome,
                })),
            );
        });
    }

    pub(super) fn state_changed(&mut self, state: AccountEncryption) {
        self.state = state;
        self.fall_back_from_unoffered_step();
        self.publish();
    }

    pub(super) fn identity_settled(&mut self, outcome: Option<&IdentityOutcome>) {
        self.setup = match outcome {
            Some(IdentityOutcome::Ready) => IdentitySetup::Settled,
            Some(IdentityOutcome::NeedsAuthentication(_)) => IdentitySetup::NeedsAuthentication,
            None => IdentitySetup::Failed,
        };
        self.publish();
    }

    pub(super) fn alert(&self) -> Option<SessionAlert> {
        self.standing().alert()
    }

    pub(super) fn open(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        tracing::debug!("opening the encryption dialog");
        self.dialog = Some(Dialog::at(EncryptionStep::Overview));
        self.publish();
    }

    pub(super) fn close(&mut self, port: Option<Arc<dyn EncryptionPort>>) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        tracing::debug!("closing the encryption dialog");
        if let (
            Step::Authenticating {
                guard: Guard::Reset,
                ..
            },
            Some(port),
        ) = (&dialog.step, port)
            && self.pending.is_none()
        {
            self.abandon_reset(port);
        }
        self.publish();
    }

    fn abandon_reset(&mut self, port: Arc<dyn EncryptionPort>) {
        self.actions.spawn(async move {
            port.abandon_identity_reset().await;
        });
    }

    pub(super) fn show_overview(&mut self) {
        let busy = self.pending.is_some();
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        let holds_typing = dialog.shows(&EncryptionStep::EnteringKey)
            || matches!(
                dialog.step,
                Step::Authenticating { .. } | Step::Exporting(_) | Step::Importing(_)
            );
        if busy && holds_typing {
            return;
        }
        if let Step::Authenticating {
            guard: Guard::Reset,
            ..
        } = dialog.step
        {
            return;
        }
        *dialog = Dialog::at(EncryptionStep::Overview);
        self.publish();
    }

    pub(super) fn enter_key(&mut self) {
        if !self.state.recovery_unlocks() || self.pending.is_some() {
            tracing::debug!("not offering the recovery key entry now");
            return;
        }
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        *dialog = Dialog::at(EncryptionStep::EnteringKey);
        self.publish();
    }

    pub(super) fn use_key(&mut self, port: Arc<dyn EncryptionPort>, key: RecoveryKey) {
        let entering = self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.shows(&EncryptionStep::EnteringKey));
        if !entering {
            tracing::debug!("dropping a recovery key typed into a closed entry");
            return;
        }
        self.act(Action::Unlock, move || async move {
            match port.recover(&key).await {
                Ok(attempt) => EncryptionOutcome::Tried(attempt),
                Err(e) => {
                    tracing::warn!("could not use the recovery key: {e}");
                    EncryptionOutcome::Failed(UserMessageKind::RecoveryKeyFailed)
                }
            }
        });
    }

    pub(super) fn verify_with_other_session(&mut self, port: Arc<dyn VerificationPort>) {
        if self.standing() != Standing::Unverified || !self.state.verified_sessions {
            tracing::debug!("no other session can verify this one now");
            return;
        }
        self.act(Action::Request, move || async move {
            match port.verify_this_session().await {
                Ok(()) => EncryptionOutcome::Requested,
                Err(e) => {
                    tracing::warn!("could not ask the other sessions to verify this one: {e}");
                    EncryptionOutcome::Failed(UserMessageKind::VerificationRequestFailed)
                }
            }
        });
    }

    pub(super) fn set_up_recovery(&mut self, port: Arc<dyn EncryptionPort>) {
        if self.standing() != Standing::Verified || self.state.recovery != Recovery::Off {
            tracing::debug!("recovery cannot be set up from here now");
            return;
        }
        self.act(Action::SetUp, move || async move {
            match port.enable_recovery().await {
                Ok(setup) => EncryptionOutcome::SetUp(setup),
                Err(e) => {
                    tracing::warn!("could not set up recovery: {e}");
                    EncryptionOutcome::Failed(UserMessageKind::RecoverySetupFailed)
                }
            }
        });
    }

    pub(super) fn change_key(&mut self, port: Arc<dyn EncryptionPort>) {
        if self.standing() != Standing::Verified || self.state.recovery != Recovery::On {
            tracing::debug!("the recovery key cannot be changed from here now");
            return;
        }
        self.act(Action::Change, move || async move {
            match port.change_recovery_key().await {
                Ok(key) => EncryptionOutcome::Changed(key),
                Err(e) => {
                    tracing::warn!("could not change the recovery key: {e}");
                    EncryptionOutcome::Failed(UserMessageKind::RecoveryKeyChangeFailed)
                }
            }
        });
    }

    pub(super) fn retry_setup(&mut self, port: Arc<dyn EncryptionPort>) {
        if !matches!(
            self.standing(),
            Standing::SetupRefused | Standing::SetupFailed
        ) || self.pending.is_some()
        {
            tracing::debug!("the account's identity needs no retry now");
            return;
        }
        self.setup = IdentitySetup::Running;
        self.act(Action::Retry, move || async move {
            EncryptionOutcome::IdentitySettled(set_up(port.as_ref(), None).await)
        });
    }

    pub(super) fn start_reset(&mut self) {
        if !self.offers_reset() || self.pending.is_some() {
            tracing::debug!("the identity cannot be reset from here now");
            return;
        }
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        *dialog = Dialog::at(EncryptionStep::ConfirmingReset);
        self.publish();
    }

    pub(super) fn reset_identity(&mut self, port: Arc<dyn EncryptionPort>) {
        let confirming = self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.shows(&EncryptionStep::ConfirmingReset));
        if !confirming || !self.offers_reset() {
            tracing::debug!("dropping an identity reset nobody confirmed");
            return;
        }
        tracing::info!("resetting the account's cryptographic identity");
        self.act(Action::Reset, move || async move {
            match port.reset_identity().await {
                Ok(guarded) => EncryptionOutcome::Reset(Some(guarded)),
                Err(e) => {
                    tracing::warn!("could not reset the account's identity: {e}");
                    EncryptionOutcome::Reset(None)
                }
            }
        });
    }

    pub(super) fn awaits_auth(&self) -> bool {
        self.dialog
            .as_ref()
            .is_some_and(|dialog| matches!(dialog.step, Step::Authenticating { .. }))
    }

    pub(super) fn answer_auth(&mut self, port: Arc<dyn EncryptionPort>, reply: Reply) {
        if self.pending.is_some() {
            tracing::debug!("an answer is already on its way to the homeserver");
            return;
        }
        let Some(Dialog {
            step: Step::Authenticating { guard, round },
            error,
        }) = self.dialog.as_mut()
        else {
            return;
        };
        let Some(answer) = round.reply(reply) else {
            tracing::debug!("that answer does not fit the step the homeserver asks for");
            return;
        };
        *error = UserMessage::default();
        let guard = *guard;
        self.authenticate(port, guard, answer);
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

    pub(super) fn cancel_auth(&mut self, port: Arc<dyn EncryptionPort>) {
        if self.pending.is_some() {
            tracing::debug!("an answer is on its way to the homeserver, not abandoning it");
            return;
        }
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        let Step::Authenticating { guard, .. } = dialog.step else {
            return;
        };
        tracing::debug!(
            ?guard,
            "abandoning the authentication the homeserver asked for"
        );
        *dialog = Dialog::at(EncryptionStep::Overview);
        if guard == Guard::Reset {
            self.abandon_reset(port);
        }
        self.publish();
    }

    pub(super) fn start_key_export(&mut self) {
        self.choose_file(KeyFile::Export);
    }

    pub(super) fn start_key_import(&mut self) {
        self.choose_file(KeyFile::Import);
    }

    pub(super) fn export_keys(&mut self, port: Arc<dyn EncryptionPort>, passphrase: KeyPassphrase) {
        let Some(Dialog {
            step: Step::Exporting(path),
            ..
        }) = self.dialog.as_ref()
        else {
            tracing::debug!("dropping a passphrase typed into a closed export");
            return;
        };
        if passphrase.expose().is_empty() {
            return;
        }
        let path = path.clone();
        self.act(Action::Export, move || async move {
            match port.export_room_keys(&path, &passphrase).await {
                Ok(count) => {
                    tracing::info!(count, "exported the room keys");
                    EncryptionOutcome::Exported(Some(count))
                }
                Err(e) => {
                    tracing::warn!("could not export the room keys: {e}");
                    EncryptionOutcome::Exported(None)
                }
            }
        });
    }

    pub(super) fn import_keys(&mut self, port: Arc<dyn EncryptionPort>, passphrase: KeyPassphrase) {
        let Some(Dialog {
            step: Step::Importing(path),
            ..
        }) = self.dialog.as_ref()
        else {
            tracing::debug!("dropping a passphrase typed into a closed import");
            return;
        };
        if passphrase.expose().is_empty() {
            return;
        }
        let path = path.clone();
        self.act(Action::Import, move || async move {
            match port.import_room_keys(&path, &passphrase).await {
                Ok(import) => {
                    tracing::info!(?import, "tried to import room keys");
                    EncryptionOutcome::Imported(Some(import))
                }
                Err(e) => {
                    tracing::warn!("could not import room keys: {e}");
                    EncryptionOutcome::Imported(None)
                }
            }
        });
    }

    pub(super) fn settled(
        &mut self,
        port: Option<Arc<dyn EncryptionPort>>,
        request: u64,
        outcome: EncryptionOutcome,
    ) {
        if request < self.first_request_of_session {
            tracing::debug!(
                request,
                "dropping an encryption action of a session that ended"
            );
            return;
        }
        let action = self
            .pending
            .take_if(|pending| pending.request == request)
            .map(|pending| pending.action);
        match outcome {
            EncryptionOutcome::Requested => self.close_quietly(),
            EncryptionOutcome::Tried(attempt) => self.tried(attempt),
            EncryptionOutcome::SetUp(RecoverySetup::Created(key))
            | EncryptionOutcome::Changed(key) => self.show_key(key),
            EncryptionOutcome::SetUp(RecoverySetup::BackupInTheWay) => {
                self.report(UserMessageKind::RecoveryBackupInTheWay);
            }
            EncryptionOutcome::IdentitySettled(outcome) => {
                self.setup_settled(port, action, outcome);
            }
            EncryptionOutcome::Reset(guarded) => self.reset_settled(port, guarded),
            EncryptionOutcome::FileChosen(file, path) => self.file_chosen(file, path),
            EncryptionOutcome::Exported(count) => self.exported(count),
            EncryptionOutcome::Imported(import) => self.imported(import),
            EncryptionOutcome::Failed(kind) => self.report(kind),
        }
        self.publish();
    }

    pub(super) async fn restart(&mut self) {
        self.actions.restart().await;
        self.first_request_of_session = self.requests.wrapping_add(1);
        self.state = AccountEncryption::default();
        self.setup = IdentitySetup::Running;
        self.dialog = None;
        self.pending = None;
    }

    pub(super) async fn shutdown(&mut self) {
        self.actions.shutdown().await;
    }

    fn standing(&self) -> Standing {
        Standing::of(self.state.trust, self.setup)
    }

    pub(super) fn is_verified(&self) -> bool {
        self.standing() == Standing::Verified
    }

    fn offers_reset(&self) -> bool {
        self.standing() == Standing::Unverified
    }

    fn setup_settled(
        &mut self,
        port: Option<Arc<dyn EncryptionPort>>,
        action: Option<Action>,
        outcome: Option<IdentityOutcome>,
    ) {
        let asked_here =
            matches!(action, Some(Action::Retry | Action::Authenticate)) && self.dialog.is_some();
        match outcome {
            Some(IdentityOutcome::NeedsAuthentication(challenge)) if asked_here => {
                self.setup = IdentitySetup::NeedsAuthentication;
                self.needs_auth(port, Guard::SetUp, *challenge);
            }
            outcome => {
                if matches!(
                    self.dialog.as_ref().map(|dialog| &dialog.step),
                    Some(Step::Authenticating {
                        guard: Guard::SetUp,
                        ..
                    })
                ) && let Some(dialog) = self.dialog.as_mut()
                {
                    *dialog = Dialog::at(EncryptionStep::Overview);
                }
                if outcome.is_none() && asked_here {
                    self.report(UserMessageKind::EncryptionSetupFailed);
                }
                self.identity_settled(outcome.as_ref());
            }
        }
    }

    fn reset_settled(&mut self, port: Option<Arc<dyn EncryptionPort>>, guarded: Option<Guarded>) {
        match guarded {
            Some(Guarded::Done) => {
                tracing::info!("reset the account's cryptographic identity");
                self.setup = IdentitySetup::Settled;
                self.dialog = Some(Dialog::at(EncryptionStep::IdentityReset));
            }
            Some(Guarded::NeedsAuth(challenge)) => self.needs_auth(port, Guard::Reset, *challenge),
            None => {
                if let Some(dialog) = self.dialog.as_mut() {
                    *dialog = Dialog::at(EncryptionStep::ConfirmingReset);
                }
                self.report(UserMessageKind::IdentityResetFailed);
            }
        }
    }

    fn needs_auth(
        &mut self,
        port: Option<Arc<dyn EncryptionPort>>,
        guard: Guard,
        challenge: AuthChallenge,
    ) {
        let Some(dialog) = self.dialog.as_ref() else {
            tracing::debug!("the dialog closed before the homeserver asked who you are");
            return;
        };
        let previous = match &dialog.step {
            Step::Authenticating { round, .. } => Some(round.as_ref()),
            Step::Shown(_) | Step::Exporting(_) | Step::Importing(_) => None,
        };
        match AuthRound::after(challenge, previous) {
            Next::Ask { round, open_page } => {
                if open_page && let Some(page) = round.page().cloned() {
                    self.launch(page);
                }
                self.set_step(Step::Authenticating {
                    guard,
                    round: Box::new(round),
                });
            }
            Next::Answer { round, answer } => {
                self.set_step(Step::Authenticating {
                    guard,
                    round: Box::new(round),
                });
                if let Some(port) = port {
                    self.authenticate(port, guard, answer);
                }
            }
            Next::Unsupported => {
                let back = match guard {
                    Guard::SetUp => EncryptionStep::Overview,
                    Guard::Reset => EncryptionStep::ConfirmingReset,
                };
                self.set_step(Step::Shown(back));
                self.report(UserMessageKind::AuthUnsupported);
            }
        }
    }

    fn authenticate(&mut self, port: Arc<dyn EncryptionPort>, guard: Guard, answer: AuthAnswer) {
        match guard {
            Guard::SetUp => self.act(Action::Authenticate, move || async move {
                EncryptionOutcome::IdentitySettled(set_up(port.as_ref(), Some(&answer)).await)
            }),
            Guard::Reset => self.act(Action::Authenticate, move || async move {
                match port.finish_identity_reset(&answer).await {
                    Ok(guarded) => EncryptionOutcome::Reset(Some(guarded)),
                    Err(e) => {
                        tracing::warn!("the identity reset failed: {e}");
                        EncryptionOutcome::Reset(None)
                    }
                }
            }),
        }
    }

    fn choose_file(&mut self, file: KeyFile) {
        if self.dialog.is_none() || self.pending.is_some() {
            return;
        }
        let files = Arc::clone(&self.files);
        self.act(Action::ChooseFile, move || async move {
            let chosen = match file {
                KeyFile::Export => files.choose_key_export(KEY_EXPORT_FILENAME).await,
                KeyFile::Import => files.pick_key_file().await,
            };
            match chosen {
                Ok(path) => EncryptionOutcome::FileChosen(file, path),
                Err(e) => {
                    tracing::warn!(?file, "the file chooser failed: {e}");
                    EncryptionOutcome::Failed(match file {
                        KeyFile::Export => UserMessageKind::KeyExportFailed,
                        KeyFile::Import => UserMessageKind::KeyImportFailed,
                    })
                }
            }
        });
    }

    fn file_chosen(&mut self, file: KeyFile, path: Option<PathBuf>) {
        let Some(path) = path else {
            tracing::debug!(?file, "no file chosen");
            return;
        };
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        dialog.error = UserMessage::default();
        dialog.step = match file {
            KeyFile::Export => Step::Exporting(path),
            KeyFile::Import => Step::Importing(path),
        };
    }

    fn exported(&mut self, count: Option<usize>) {
        let Some(count) = count else {
            self.report(UserMessageKind::KeyExportFailed);
            return;
        };
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        let file = match &dialog.step {
            Step::Exporting(path) => file_label(path),
            Step::Shown(_) | Step::Authenticating { .. } | Step::Importing(_) => String::new(),
        };
        *dialog = Dialog::at(EncryptionStep::KeysExported { count, file });
    }

    fn imported(&mut self, import: Option<KeyImport>) {
        match import {
            Some(KeyImport::Imported { imported, total }) => {
                if let Some(dialog) = self.dialog.as_mut() {
                    *dialog = Dialog::at(EncryptionStep::KeysImported { imported, total });
                }
            }
            Some(KeyImport::WrongPassphrase) => self.report(UserMessageKind::KeyPassphraseWrong),
            Some(KeyImport::NotAKeyFile) => self.report(UserMessageKind::KeyFileUnreadable),
            None => self.report(UserMessageKind::KeyImportFailed),
        }
    }

    fn tried(&mut self, attempt: RecoveryAttempt) {
        let step = match attempt {
            RecoveryAttempt::Unlocked => EncryptionStep::Unlocked,
            RecoveryAttempt::UnlockedWithoutBackup => EncryptionStep::UnlockedWithoutBackup,
            RecoveryAttempt::WrongKey => {
                self.report(UserMessageKind::RecoveryKeyWrong);
                return;
            }
        };
        if let Some(dialog) = self.dialog.as_mut() {
            *dialog = Dialog::at(step);
        }
    }

    fn show_key(&mut self, key: RecoveryKey) {
        if self.dialog.is_none() {
            tracing::info!("reopening the encryption dialog to show the new recovery key");
        }
        self.dialog = Some(Dialog::at(EncryptionStep::ShowingKey(key)));
    }

    fn close_quietly(&mut self) {
        if self.dialog.take().is_some() {
            tracing::debug!("closing the encryption dialog");
        }
    }

    fn report(&mut self, kind: UserMessageKind) {
        match self.dialog.as_mut() {
            Some(dialog) => dialog.error = UserMessage::new(kind),
            None => show_toast(self.output.as_ref(), Toast::Error(UserMessage::new(kind))),
        }
    }

    fn set_step(&mut self, step: Step) {
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.step = step;
        }
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

    fn fall_back_from_unoffered_step(&mut self) {
        let unlocks = self.state.recovery_unlocks();
        let offers_reset = self.offers_reset();
        let idle = self.pending.is_none();
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        let unoffered = (dialog.shows(&EncryptionStep::EnteringKey) && !unlocks)
            || (dialog.shows(&EncryptionStep::ConfirmingReset) && !offers_reset);
        if unoffered && idle {
            *dialog = Dialog::at(EncryptionStep::Overview);
        }
    }

    fn act<F, Fut>(&mut self, action: Action, work: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = EncryptionOutcome> + Send,
    {
        if let Some(pending) = &self.pending {
            tracing::debug!(running = ?pending.action, ?action, "an encryption action is already running");
            return;
        }
        self.requests = self.requests.wrapping_add(1);
        let request = self.requests;
        self.pending = Some(Pending { request, action });
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.error = UserMessage::default();
        }
        self.publish();

        let events = self.events.clone();
        self.actions.spawn(async move {
            let outcome = work().await;
            drop(
                events.send(AppEvent::Encryption(EncryptionEvent::ActionSettled {
                    request,
                    outcome,
                })),
            );
        });
    }

    fn publish(&self) {
        let activity = self
            .pending
            .as_ref()
            .map_or(EncryptionActivity::None, |pending| {
                pending.action.activity()
            });
        let view = EncryptionView {
            standing: self.standing(),
            verified_sessions: self.state.verified_sessions,
            recovery: self.state.recovery,
            key_storage: self.state.key_storage,
            dialog: self.dialog.as_ref().map(|dialog| EncryptionDialog {
                step: shown_step(&dialog.step),
                activity,
                error: dialog.error.clone(),
            }),
        };
        self.output
            .publish(Box::new(move |state| state.encryption = view));
    }
}

fn shown_step(step: &Step) -> EncryptionStep {
    match step {
        Step::Shown(step) => step.clone(),
        Step::Authenticating {
            guard: Guard::SetUp,
            round,
        } if round.is_silent() => EncryptionStep::Overview,
        Step::Authenticating {
            guard: Guard::Reset,
            round,
        } if round.is_silent() => EncryptionStep::ConfirmingReset,
        Step::Authenticating {
            guard: Guard::SetUp,
            round,
        } => EncryptionStep::AuthenticatingSetup(round.prompt()),
        Step::Authenticating {
            guard: Guard::Reset,
            round,
        } => EncryptionStep::AuthenticatingReset(round.prompt()),
        Step::Exporting(path) => EncryptionStep::ExportingKeys(file_label(path)),
        Step::Importing(path) => EncryptionStep::ImportingKeys(file_label(path)),
    }
}

fn file_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

async fn set_up(port: &dyn EncryptionPort, auth: Option<&AuthAnswer>) -> Option<IdentityOutcome> {
    match port.set_up_identity(auth).await {
        Ok(outcome) => Some(outcome),
        Err(e) => {
            tracing::warn!("could not set up the account's cross-signing identity: {e}");
            None
        }
    }
}

async fn follow(port: &dyn EncryptionPort, state_tx: mpsc::Sender<AccountEncryption>) {
    if let Err(e) = port.follow(state_tx).await {
        tracing::warn!("failed to follow this session's encryption: {e}");
    }
}

async fn forward_states(
    events: &EventSender,
    session: u64,
    mut state_rx: mpsc::Receiver<AccountEncryption>,
) {
    while let Some(state) = state_rx.recv().await {
        let event = AppEvent::Encryption(EncryptionEvent::Changed { session, state });
        if events.send(event).is_err() {
            return;
        }
    }
}
