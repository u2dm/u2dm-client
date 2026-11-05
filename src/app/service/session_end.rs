use std::sync::Arc;

use super::{AppService, Ending, HeldSession};
use crate::app::calls::Calls;
use crate::app::event::EndReason;
use crate::app::presence::Presence;
use crate::app::selection::Selection;
use crate::app::session;
use crate::app::session::SuspendedSession;
use crate::app::task_group::TaskGroup;
use crate::commands::effects::Effect;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::UiCommand;
use crate::commands::view::{AppViewState, LoginActivity};
use crate::domain::account::AccountScope;
use crate::domain::sync::ReauthReason;
use crate::ports::matrix::CleanupReport;

async fn say_goodbye_then_stop_syncing(
    presence: &mut Presence,
    calls: &mut Calls,
    background: &mut TaskGroup,
) {
    tokio::join!(presence.shutdown(), calls.shutdown());
    background.shutdown().await;
}

macro_rules! wind_controllers {
    ($service:ident, $first:expr, $wind:ident, $media:ident) => {
        tokio::join!(
            $first,
            $service.active_timeline.shutdown(),
            $service.pinned.$wind(),
            $service.identity_alerts.$wind(),
            $service.encryption.$wind(),
            $service.account.$wind(),
            $service.operations.$wind(),
            $service.send_lanes.$wind(),
            $service.media.$media(),
            $service.audio.$wind(),
            $service.video.$wind(),
            $service.packs.$wind(),
            $service.space_index.$wind(),
            $service.invites.$wind(),
            $service.removals.$wind(),
            $service.space_leave.$wind(),
            $service.upgrades.$wind(),
            $service.room_previews.$wind(),
            $service.room_creation.$wind(),
            $service.invite_people.$wind(),
            $service.typing.$wind(),
            $service.room_info.$wind(),
            $service.user_info.$wind(),
            $service.readers.$wind(),
            $service.mentions.$wind(),
            $service.room_log.$wind(),
        )
    };
}

impl AppService {
    pub(super) async fn handle_logout_command(&mut self, cmd: UiCommand) {
        match cmd {
            UiCommand::Logout => match self.port(|a| &a.account) {
                Some(port) => self.account.check_logout(port),
                None => self.end_session(EndReason::UserLogout).await,
            },
            UiCommand::ConfirmLogout => {
                if self.account.confirm_logout() {
                    self.end_session(EndReason::UserLogout).await;
                }
            }
            UiCommand::CancelLogout => self.account.cancel_logout(),
            other => tracing::warn!(command = %other, "this command is not about logging out"),
        }
    }

    pub(super) fn settle_erasure_start(&mut self, session: u64) {
        if self.lifecycle.begin_cleanup(session) {
            self.session.set_activity(LoginActivity::CleaningUp);
        }
    }

    pub(super) fn settle_erasure(
        &mut self,
        session: u64,
        reason: EndReason,
        left_behind: Option<&str>,
        report: &CleanupReport,
    ) {
        if !self.lifecycle.finish_logout(session) {
            tracing::debug!("cleanup finished for a superseded session, dropping");
            return;
        }
        let mut messages = match reason {
            EndReason::Expired => vec![UserMessage::new(UserMessageKind::SessionExpired)],
            EndReason::Deactivated => vec![UserMessage::new(UserMessageKind::AccountDeactivated)],
            EndReason::UserLogout => Vec::new(),
        };
        messages.extend(left_behind.map(session::left_behind_notice));
        messages.extend(session::cleanup_problem(report));
        self.session.settle_logout(messages);
    }

    async fn shutdown_all_tasks(&mut self) {
        tokio::join!(self.presence.restart(), self.calls.restart());
        wind_controllers!(self, self.background.shutdown(), restart, cancel_and_drain);
    }

    pub(super) async fn end_session(&mut self, reason: EndReason) {
        let Some(session) = self.lifecycle.begin_logout() else {
            return;
        };
        if matches!(reason, EndReason::Expired) {
            tracing::info!("session expired, clearing local state");
        }
        match (self.held_session.ending(), reason) {
            (Some(Ending::Live(ending)), EndReason::UserLogout) => {
                self.presence.sign_off();
                self.calls.hang_up();
                self.tear_down_session(Box::new(AppViewState::signing_out()))
                    .await;
                self.session
                    .spawn_logout(&mut self.operations, session, ending);
            }
            (Some(Ending::Detached(ending)), EndReason::UserLogout) => {
                tracing::info!(
                    device_id = %ending.device_id,
                    "erasing a suspended session without a homeserver logout, so the homeserver keeps this device"
                );
                self.tear_down_session(Box::new(AppViewState::logged_out()))
                    .await;
                let left_behind = Some(ending.device_id.clone());
                self.session.spawn_erase_session(
                    &mut self.operations,
                    session,
                    ending,
                    reason,
                    left_behind,
                );
            }
            (
                Some(Ending::Live(ending) | Ending::Detached(ending)),
                EndReason::Expired | EndReason::Deactivated,
            ) => {
                self.tear_down_session(Box::new(AppViewState::logged_out()))
                    .await;
                self.session.spawn_erase_session(
                    &mut self.operations,
                    session,
                    ending,
                    reason,
                    None,
                );
            }
            (None, _) => {
                self.tear_down_session(Box::new(AppViewState::logged_out()))
                    .await;
                self.lifecycle.finish_logout(session);
            }
        }
    }

    pub(super) async fn suspend_session(&mut self, reason: ReauthReason) {
        let Some((session, lifecycle_port)) = self
            .held_session
            .running()
            .map(|a| (a.session.clone(), Arc::clone(&a.lifecycle)))
        else {
            return;
        };
        if !self.lifecycle.suspend() {
            return;
        }
        tracing::info!(
            user_id = %session.user_id,
            device_id = %session.device_id,
            ?reason,
            "the homeserver refused this session's token, keeping this device and its local data"
        );
        self.tear_down_session(Box::new(AppViewState::reauthenticating(&session, reason)))
            .await;
        lifecycle_port.suspend().await;
        self.held_session = HeldSession::Suspended(Box::new(SuspendedSession {
            account: AccountScope::from_session(&session),
            session,
            lifecycle: lifecycle_port,
        }));
    }

    async fn tear_down_session(&mut self, view: Box<AppViewState>) {
        self.attachments.clear();
        self.submissions.forget_all();
        self.message_actions.forget();
        self.event_source.reset();
        self.output.emit(Effect::SessionReset(view.clone())).await;
        self.output.replace(*view);
        self.shutdown_all_tasks().await;
        self.media.clear_session().await;
        self.room_directory.reset();
        self.verification.reset();
        self.notifications.reset();
        self.selection = Selection::default();
        self.last_selected_room = None;
        self.held_session = HeldSession::None;
        self.room_log.forget();
    }

    pub(super) async fn handle_quit(&mut self) {
        self.presence.sign_off();
        let farewell = Box::pin(say_goodbye_then_stop_syncing(
            &mut self.presence,
            &mut self.calls,
            &mut self.background,
        ));
        wind_controllers!(self, farewell, shutdown, drain);
    }
}
