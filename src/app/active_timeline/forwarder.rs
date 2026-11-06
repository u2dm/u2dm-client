use std::future::Future;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::{report_delete_failure, report_poll_failure};
use crate::app::event::{AppEvent, TimelineEvent};
use crate::app::input::EventSender;
use crate::app::show_toast;
use crate::commands::effects::Effect;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::Toast;
use crate::domain::message::MessageEdit;
use crate::domain::room::RoomId;
use crate::domain::timeline::{
    Arrivals, JumpTarget, TimelineAdvance, TimelineFocus, TimelinePatch, TimelineStatus,
    TimelineUpdate,
};
use crate::error::{AppError, Result};
use crate::ports::output::AppOutputPort;

pub(super) struct Forwarder {
    pub(super) output: Arc<dyn AppOutputPort>,
    pub(super) events: EventSender,
    pub(super) room_id: RoomId,
    pub(super) generation: i32,
    pub(super) focus: TimelineFocus,
    pub(super) next_snapshot: Snapshot,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Snapshot {
    Opening,
    Replacement,
}

impl Forwarder {
    pub(super) async fn run(&mut self, rx: &mut mpsc::Receiver<TimelineUpdate>) {
        while let Some(update) = rx.recv().await {
            tracing::debug!(
                update = update.label(),
                room_id = %self.room_id,
                "forwarding timeline update"
            );
            if !self.dispatch(update).await {
                break;
            }
        }
    }

    pub(super) async fn report_failures_until(
        &self,
        subscription: impl Future + Send,
        rx: &mut mpsc::Receiver<TimelineUpdate>,
    ) {
        tokio::select! {
            _ = subscription => {}
            () = self.report_failures(rx) => {}
        }
        while let Ok(update) = rx.try_recv() {
            self.report_failure(update);
        }
    }

    async fn report_failures(&self, rx: &mut mpsc::Receiver<TimelineUpdate>) {
        while let Some(update) = rx.recv().await {
            self.report_failure(update);
        }
    }

    fn report_failure(&self, update: TimelineUpdate) {
        match update {
            TimelineUpdate::PollSendFailed(action) => {
                report_poll_failure(self.output.as_ref(), action);
            }
            TimelineUpdate::EditUnsaved(edit) => {
                self.offer_unsaved_edit(edit);
            }
            TimelineUpdate::DeleteFailed => report_delete_failure(self.output.as_ref()),
            TimelineUpdate::Patch { .. }
            | TimelineUpdate::ResolvingUnread
            | TimelineUpdate::UnreadUnresolved
            | TimelineUpdate::Pagination { .. }
            | TimelineUpdate::JumpOutcome { .. }
            | TimelineUpdate::AudioLocated { .. }
            | TimelineUpdate::SourceLocated { .. }
            | TimelineUpdate::ReadersLocated { .. } => {}
        }
    }

    fn offer_unsaved_edit(&self, edit: MessageEdit) -> bool {
        let unsaved = AppEvent::EditUnsaved {
            room_id: self.room_id.clone(),
            edit,
        };
        self.events.send(unsaved).is_ok()
    }

    async fn dispatch(&mut self, update: TimelineUpdate) -> bool {
        match update {
            TimelineUpdate::Patch { patch, arrivals } => self.forward_patch(patch, arrivals).await,
            TimelineUpdate::ResolvingUnread => {
                self.emit_status(TimelineStatus::LoadingUnread).await;
            }
            TimelineUpdate::UnreadUnresolved => {
                self.send_advance(TimelineAdvance::UnreadUnresolved);
            }
            TimelineUpdate::JumpOutcome { event_id, target } => {
                self.forward_jump(event_id, target).await;
            }
            TimelineUpdate::PollSendFailed(action) => {
                report_poll_failure(self.output.as_ref(), action);
            }
            TimelineUpdate::EditUnsaved(edit) => {
                if !self.offer_unsaved_edit(edit) {
                    return false;
                }
            }
            TimelineUpdate::DeleteFailed => report_delete_failure(self.output.as_ref()),
            TimelineUpdate::AudioLocated { request, track } => {
                return self.tell(|room_id, generation| TimelineEvent::AudioLocated {
                    room_id,
                    generation,
                    request,
                    track,
                });
            }
            TimelineUpdate::SourceLocated { request, source } => {
                return self.tell(|room_id, generation| TimelineEvent::SourceLocated {
                    room_id,
                    generation,
                    request,
                    source,
                });
            }
            TimelineUpdate::ReadersLocated { request, readers } => {
                return self.tell(|room_id, generation| TimelineEvent::ReadersLocated {
                    room_id,
                    generation,
                    request,
                    readers,
                });
            }
            TimelineUpdate::Pagination { direction, outcome } => {
                return self.tell(|room_id, generation| TimelineEvent::PaginationCompleted {
                    room_id,
                    generation,
                    direction,
                    outcome,
                });
            }
        }
        true
    }

    async fn forward_patch(&mut self, patch: Box<TimelinePatch>, arrivals: Arrivals) {
        if let Some(advance) = read_position_advance(patch.as_ref(), arrivals, self.next_snapshot) {
            self.send_advance(advance);
        }
        if patch.opens_room() {
            self.next_snapshot = Snapshot::Replacement;
        }
        self.output
            .emit(Effect::Timeline {
                room_id: self.room_id.clone(),
                generation: self.generation,
                patch,
            })
            .await;
    }

    async fn forward_jump(&self, event_id: String, target: JumpTarget) {
        let row = match target {
            JumpTarget::Row(row) => row,
            JumpTarget::NotLoaded => {
                self.widen_search_for(event_id);
                return;
            }
            JumpTarget::NotRenderable => {
                show_toast(
                    self.output.as_ref(),
                    Toast::Error(UserMessage::new(UserMessageKind::MessageNotShowable)),
                );
                return;
            }
        };
        self.send_advance(TimelineAdvance::Focused);
        self.output
            .emit(Effect::TimelineFocus {
                room_id: self.room_id.clone(),
                generation: self.generation,
                event_id,
                row,
            })
            .await;
    }

    fn tell(&self, event: impl FnOnce(RoomId, i32) -> TimelineEvent) -> bool {
        let event = event(self.room_id.clone(), self.generation);
        self.events.send(AppEvent::Timeline(event)).is_ok()
    }

    fn send_advance(&self, advance: TimelineAdvance) {
        self.tell(|room_id, generation| TimelineEvent::Advanced {
            room_id,
            generation,
            advance,
        });
    }

    pub(super) async fn subscription_ended(&self, result: Result<()>) {
        match result {
            Err(AppError::FocusedEventUnavailable) => {
                tracing::info!(
                    "the message to open around cannot be loaded, opening the room live"
                );
                self.fall_back_to_live();
            }
            Err(e) => {
                tracing::warn!("timeline subscription failed: {e}");
                self.emit_status(TimelineStatus::Failed { retryable: true })
                    .await;
            }
            Ok(()) => {
                tracing::debug!("timeline subscription ended");
                self.emit_status(TimelineStatus::Disconnected).await;
            }
        }
    }

    async fn emit_status(&self, status: TimelineStatus) {
        self.output
            .emit(Effect::TimelineStatus {
                room_id: self.room_id.clone(),
                generation: self.generation,
                status,
            })
            .await;
    }

    fn fall_back_to_live(&self) {
        show_toast(
            self.output.as_ref(),
            Toast::Error(UserMessage::new(UserMessageKind::MessageNotFound)),
        );
        self.refocus(TimelineFocus::ReadPosition);
    }

    fn widen_search_for(&self, event_id: String) {
        let window_opened_around_it = self.focus.target() == Some(event_id.as_str());
        if window_opened_around_it {
            show_toast(
                self.output.as_ref(),
                Toast::Error(UserMessage::new(UserMessageKind::MessageNotFound)),
            );
        } else {
            self.refocus(TimelineFocus::Event(event_id));
        }
    }

    fn refocus(&self, focus: TimelineFocus) {
        self.tell(|room_id, generation| TimelineEvent::Refocus {
            room_id,
            generation,
            focus,
        });
    }
}

fn read_position_advance(
    patch: &TimelinePatch,
    arrivals: Arrivals,
    snapshot: Snapshot,
) -> Option<TimelineAdvance> {
    if snapshot == Snapshot::Opening
        && let Some(anchor) = patch.unread_anchor()
    {
        return Some(TimelineAdvance::Anchored {
            count: anchor.count,
        });
    }

    let opens_room = patch.opens_room();
    if arrivals.is_silent() && !opens_room {
        return None;
    }
    Some(TimelineAdvance::Arrived {
        arrivals,
        opens_room,
    })
}
