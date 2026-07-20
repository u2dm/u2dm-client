use std::sync::{Arc, Mutex as StdMutex, MutexGuard, PoisonError};

use crate::commands::UiCommand;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AppPhase {
    Restoring,
    LoggedOut,
    Authenticating,
    Syncing,
    LoggingOut,
}

struct Inner {
    phase: AppPhase,
    attempt: u64,
    session: u64,
}

#[derive(Clone)]
pub(super) struct Lifecycle {
    inner: Arc<StdMutex<Inner>>,
}

impl Lifecycle {
    pub(super) fn new() -> Self {
        Self {
            inner: Arc::new(StdMutex::new(Inner {
                phase: AppPhase::Restoring,
                attempt: 0,
                session: 0,
            })),
        }
    }

    fn guard(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn phase(&self) -> AppPhase {
        self.guard().phase
    }

    pub(super) fn begin_auth(&self) -> u64 {
        let mut inner = self.guard();
        inner.attempt += 1;
        inner.phase = AppPhase::Authenticating;
        inner.attempt
    }

    pub(super) fn settle_auth(&self, attempt: u64) -> bool {
        let mut inner = self.guard();
        if inner.phase == AppPhase::Authenticating && inner.attempt == attempt {
            inner.phase = AppPhase::LoggedOut;
            true
        } else {
            false
        }
    }

    pub(super) fn cancel_auth(&self) -> bool {
        let mut inner = self.guard();
        if inner.phase == AppPhase::Authenticating {
            inner.phase = AppPhase::LoggedOut;
            true
        } else {
            false
        }
    }

    pub(super) fn promote_to_syncing(&self, attempt: u64) -> Option<u64> {
        let mut inner = self.guard();
        if inner.phase == AppPhase::Authenticating && inner.attempt == attempt {
            inner.phase = AppPhase::Syncing;
            inner.session += 1;
            Some(inner.session)
        } else {
            None
        }
    }

    pub(super) fn restore_succeeded(&self) -> Option<u64> {
        let mut inner = self.guard();
        if inner.phase == AppPhase::Restoring {
            inner.phase = AppPhase::Syncing;
            inner.session += 1;
            Some(inner.session)
        } else {
            None
        }
    }

    pub(super) fn restore_failed(&self) -> bool {
        let mut inner = self.guard();
        if inner.phase == AppPhase::Restoring {
            inner.phase = AppPhase::LoggedOut;
            true
        } else {
            false
        }
    }

    pub(super) fn begin_logout(&self) -> Option<u64> {
        let mut inner = self.guard();
        if inner.phase == AppPhase::Syncing {
            inner.phase = AppPhase::LoggingOut;
            Some(inner.session)
        } else {
            None
        }
    }

    pub(super) fn finish_logout(&self, session: u64) -> bool {
        let mut inner = self.guard();
        if inner.phase == AppPhase::LoggingOut && inner.session == session {
            inner.phase = AppPhase::LoggedOut;
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
        UiCommand::CheckServer(_) | UiCommand::LoginPassword(_) | UiCommand::LoginOAuth => {
            phase == AppPhase::LoggedOut
        }
        UiCommand::CancelOAuth => phase == AppPhase::Authenticating,
        _ => phase == AppPhase::Syncing,
    }
}
