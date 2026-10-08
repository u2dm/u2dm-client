use crate::commands::ui::UiCommand;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AppPhase {
    Starting,
    Restoring,
    RestorePaused,
    Blocked,
    LoggedOut,
    Authenticating,
    CancellingAuth,
    Syncing,
    SoftLoggedOut,
    Reauthenticating,
    CancellingReauth,
    LoggingOut,
    CleaningUp,
}

pub(super) enum Settled {
    Awaited,
    Cancelled,
}

pub(super) struct Lifecycle {
    phase: AppPhase,
    attempt: u64,
    session: u64,
}

impl Lifecycle {
    pub(super) fn new() -> Self {
        Self {
            phase: AppPhase::Starting,
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

    pub(super) fn is_restoring(&self) -> bool {
        self.phase == AppPhase::Restoring
    }

    pub(super) fn begin_restore(&mut self) {
        self.phase = AppPhase::Restoring;
    }

    pub(super) fn pause_restore(&mut self) -> bool {
        if self.phase == AppPhase::Restoring {
            self.phase = AppPhase::RestorePaused;
            true
        } else {
            false
        }
    }

    pub(super) fn begin_auth(&mut self) -> u64 {
        self.attempt = self.attempt.saturating_add(1);
        self.phase = AppPhase::Authenticating;
        self.attempt
    }

    pub(super) fn begin_reauth(&mut self) -> Option<u64> {
        if self.phase != AppPhase::SoftLoggedOut {
            return None;
        }
        self.attempt = self.attempt.saturating_add(1);
        self.phase = AppPhase::Reauthenticating;
        Some(self.attempt)
    }

    fn idle_after_auth(phase: AppPhase) -> Option<(AppPhase, Settled)> {
        match phase {
            AppPhase::Authenticating => Some((AppPhase::LoggedOut, Settled::Awaited)),
            AppPhase::CancellingAuth => Some((AppPhase::LoggedOut, Settled::Cancelled)),
            AppPhase::Reauthenticating => Some((AppPhase::SoftLoggedOut, Settled::Awaited)),
            AppPhase::CancellingReauth => Some((AppPhase::SoftLoggedOut, Settled::Cancelled)),
            _ => None,
        }
    }

    pub(super) fn settle_auth(&mut self, attempt: u64) -> Option<Settled> {
        let (idle, settled) =
            Self::idle_after_auth(self.phase).filter(|_| self.attempt == attempt)?;
        self.phase = idle;
        Some(settled)
    }

    pub(super) fn awaits(&self, attempt: u64) -> bool {
        self.attempt == attempt
            && matches!(self.phase, AppPhase::Authenticating | AppPhase::Reauthenticating)
    }

    pub(super) fn runs(&self, session: u64) -> bool {
        self.phase == AppPhase::Syncing && self.session == session
    }

    pub(super) fn cancel_auth(&mut self) -> bool {
        let cancelling = match self.phase {
            AppPhase::Authenticating => AppPhase::CancellingAuth,
            AppPhase::Reauthenticating => AppPhase::CancellingReauth,
            _ => return false,
        };
        self.phase = cancelling;
        true
    }

    pub(super) fn promote_to_syncing(&mut self, attempt: u64) -> Option<u64> {
        if self.phase == AppPhase::Authenticating && self.attempt == attempt {
            self.phase = AppPhase::Syncing;
            self.session = self.session.saturating_add(1);
            Some(self.session)
        } else {
            None
        }
    }

    pub(super) fn suspend(&mut self) -> bool {
        if self.phase != AppPhase::Syncing {
            return false;
        }
        self.phase = AppPhase::SoftLoggedOut;
        true
    }

    pub(super) fn resume_syncing(&mut self, attempt: u64) -> Option<u64> {
        if self.phase == AppPhase::Reauthenticating && self.attempt == attempt {
            self.phase = AppPhase::Syncing;
            self.session = self.session.saturating_add(1);
            Some(self.session)
        } else {
            None
        }
    }

    pub(super) fn restore_succeeded(&mut self) -> Option<u64> {
        if self.phase == AppPhase::Restoring {
            self.phase = AppPhase::Syncing;
            self.session = self.session.saturating_add(1);
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
        if matches!(self.phase, AppPhase::Syncing | AppPhase::SoftLoggedOut) {
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
        UiCommand::RestoreSession => matches!(phase, AppPhase::Starting | AppPhase::RestorePaused),
        UiCommand::CheckServer(_)
        | UiCommand::LoginPassword(_)
        | UiCommand::LoginOAuth
        | UiCommand::BackToHomeserver => phase == AppPhase::LoggedOut,
        UiCommand::ReauthPassword(_) | UiCommand::ReauthOAuth => phase == AppPhase::SoftLoggedOut,
        UiCommand::CancelOAuth => {
            matches!(phase, AppPhase::Authenticating | AppPhase::Reauthenticating)
        }
        UiCommand::Logout => matches!(phase, AppPhase::Syncing | AppPhase::SoftLoggedOut),
        UiCommand::SelectSpace(_)
        | UiCommand::SelectDirect
        | UiCommand::SelectSubspace(_)
        | UiCommand::MoveSpace { .. }
        | UiCommand::SelectRoom(_)
        | UiCommand::FilterRooms(_)
        | UiCommand::OpenSpace(_)
        | UiCommand::OpenSpaceIndex
        | UiCommand::CloseSpaceIndex
        | UiCommand::PageSpaceIndex
        | UiCommand::RetrySpaceIndex
        | UiCommand::JoinSpaceChild(_)
        | UiCommand::OpenSpaceChild(_)
        | UiCommand::OpenRoomInfo(_)
        | UiCommand::CloseRoomInfo
        | UiCommand::ShowRoomInfoPane
        | UiCommand::HideRoomInfoPane
        | UiCommand::PageRoomMembers
        | UiCommand::RetryRoomMembers
        | UiCommand::FilterRoomMembers(_)
        | UiCommand::SetRoomNotify { .. }
        | UiCommand::LeaveRoom(_)
        | UiCommand::OpenRoomMenu(_)
        | UiCommand::CloseRoomMenu
        | UiCommand::MarkRoomRead(_)
        | UiCommand::CopyRoomLink(_)
        | UiCommand::OpenRoomLog(_)
        | UiCommand::CloseRoomLog
        | UiCommand::OpenUserInfo(_)
        | UiCommand::CloseUserInfo
        | UiCommand::RetryUserInfo
        | UiCommand::MessageUser(_)
        | UiCommand::IgnoreUser(_)
        | UiCommand::UnignoreUser(_)
        | UiCommand::KickUser(_)
        | UiCommand::BanUser(_)
        | UiCommand::UnbanUser(_)
        | UiCommand::SendMessage { .. }
        | UiCommand::EditMessage { .. }
        | UiCommand::SuggestMentions { .. }
        | UiCommand::EndMentions
        | UiCommand::DismissUnsent { .. }
        | UiCommand::PickAttachment { .. }
        | UiCommand::SendAttachment { .. }
        | UiCommand::CancelAttachment
        | UiCommand::SendSticker { .. }
        | UiCommand::SendPoll { .. }
        | UiCommand::PaginateBackwards { .. }
        | UiCommand::PaginateForwards { .. }
        | UiCommand::JumpToLatest { .. }
        | UiCommand::JumpToEvent { .. }
        | UiCommand::OpenPinned { .. }
        | UiCommand::PreviousPinned { .. }
        | UiCommand::NextPinned { .. }
        | UiCommand::CopyMessageLink { .. }
        | UiCommand::OpenEventSource { .. }
        | UiCommand::CloseEventSource
        | UiCommand::OpenReaders { .. }
        | UiCommand::CloseReaders
        | UiCommand::PageReaders
        | UiCommand::PinMessage { .. }
        | UiCommand::UnpinMessage { .. }
        | UiCommand::DeleteMessage { .. }
        | UiCommand::ToggleReaction { .. }
        | UiCommand::VotePoll { .. }
        | UiCommand::EndPoll { .. }
        | UiCommand::EditPoll { .. }
        | UiCommand::RetrySend { .. }
        | UiCommand::DiscardSend { .. }
        | UiCommand::RetryTimeline
        | UiCommand::AcceptVerification
        | UiCommand::RejectVerification
        | UiCommand::ConfirmVerification
        | UiCommand::DismissVerification
        | UiCommand::OpenMedia { .. }
        | UiCommand::OpenVideo { .. }
        | UiCommand::CloseVideo
        | UiCommand::PlayAudio { .. }
        | UiCommand::CloseAudio
        | UiCommand::AudioEnded { .. }
        | UiCommand::OpenLink { .. }
        | UiCommand::SaveFile { .. }
        | UiCommand::DismissToast => phase == AppPhase::Syncing,
    }
}
