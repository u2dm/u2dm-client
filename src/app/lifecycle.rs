use crate::commands::ui::UiCommand;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AppPhase {
    Restoring,
    Blocked,
    LoggedOut,
    Authenticating,
    Syncing,
    LoggingOut,
    CleaningUp,
}

pub(super) struct Lifecycle {
    phase: AppPhase,
    attempt: u64,
    session: u64,
}

impl Lifecycle {
    pub(super) fn new() -> Self {
        Self {
            phase: AppPhase::Restoring,
            attempt: 0,
            session: 0,
        }
    }

    pub(super) fn phase(&self) -> AppPhase {
        self.phase
    }

    pub(super) fn block(&mut self) {
        self.phase = AppPhase::Blocked;
    }

    pub(super) fn is_logged_out(&self) -> bool {
        self.phase == AppPhase::LoggedOut
    }

    pub(super) fn is_restoring(&self) -> bool {
        self.phase == AppPhase::Restoring
    }

    pub(super) fn begin_auth(&mut self) -> u64 {
        self.attempt += 1;
        self.phase = AppPhase::Authenticating;
        self.attempt
    }

    pub(super) fn settle_auth(&mut self, attempt: u64) -> bool {
        if self.phase == AppPhase::Authenticating && self.attempt == attempt {
            self.phase = AppPhase::LoggedOut;
            true
        } else {
            false
        }
    }

    pub(super) fn is_current_attempt(&self, attempt: u64) -> bool {
        self.attempt == attempt
    }

    pub(super) fn cancel_auth(&mut self) -> bool {
        if self.phase == AppPhase::Authenticating {
            self.phase = AppPhase::LoggedOut;
            true
        } else {
            false
        }
    }

    pub(super) fn promote_to_syncing(&mut self, attempt: u64) -> Option<u64> {
        if self.phase == AppPhase::Authenticating && self.attempt == attempt {
            self.phase = AppPhase::Syncing;
            self.session += 1;
            Some(self.session)
        } else {
            None
        }
    }

    pub(super) fn restore_succeeded(&mut self) -> Option<u64> {
        if self.phase == AppPhase::Restoring {
            self.phase = AppPhase::Syncing;
            self.session += 1;
            Some(self.session)
        } else {
            None
        }
    }

    pub(super) fn restore_failed(&mut self) -> bool {
        if self.phase == AppPhase::Restoring {
            self.phase = AppPhase::LoggedOut;
            true
        } else {
            false
        }
    }

    pub(super) fn begin_logout(&mut self) -> Option<u64> {
        if self.phase == AppPhase::Syncing {
            self.phase = AppPhase::LoggingOut;
            Some(self.session)
        } else {
            None
        }
    }

    pub(super) fn begin_cleanup(&mut self, session: u64) -> bool {
        if self.phase == AppPhase::LoggingOut && self.session == session {
            self.phase = AppPhase::CleaningUp;
            true
        } else {
            false
        }
    }

    pub(super) fn finish_logout(&mut self, session: u64) -> bool {
        let ending = self.phase == AppPhase::LoggingOut || self.phase == AppPhase::CleaningUp;
        if ending && self.session == session {
            self.phase = AppPhase::LoggedOut;
            true
        } else {
            false
        }
    }
}

pub(super) fn command_allowed(phase: AppPhase, cmd: &UiCommand) -> bool {
    match cmd {
        UiCommand::Quit => true,
        UiCommand::RestoreSession => phase == AppPhase::Restoring,
        UiCommand::CheckServer(_)
        | UiCommand::LoginPassword(_)
        | UiCommand::LoginOAuth
        | UiCommand::BackToHomeserver => phase == AppPhase::LoggedOut,
        UiCommand::CancelOAuth => phase == AppPhase::Authenticating,
        _ => phase == AppPhase::Syncing,
    }
}
