use std::sync::Arc;

use super::RoomInfo;
use crate::app::event::{ActionOutcome, AppEvent, RoomActionEvent};
use crate::app::show_toast;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{CopiedLink, Toast};
use crate::domain::room::{NotifyMode, Room, RoomId};
use crate::ports::matrix::{LeavePort, RoomInfoPort};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NotifyChange {
    Requested { request: u64, target: NotifyMode },
    AwaitingEcho { target: NotifyMode },
}

impl NotifyChange {
    fn target(self) -> NotifyMode {
        match self {
            Self::Requested { target, .. } | Self::AwaitingEcho { target } => target,
        }
    }
}

impl RoomInfo {
    pub(in crate::app) fn set_notify(
        &mut self,
        port: Arc<dyn RoomInfoPort>,
        room: Option<&Room>,
        mode: NotifyMode,
    ) {
        let Some(room) = room else {
            tracing::debug!(?mode, "the room is not listed, not changing its mode");
            return;
        };
        let room_id = room.id.clone();
        let change = self.notify.get(&room_id).copied();
        if self.leaving.contains(&room_id)
            || matches!(change, Some(NotifyChange::Requested { .. }))
            || change.map_or(room.notify, NotifyChange::target) == mode
        {
            tracing::debug!(%room_id, ?mode, "ignoring a notification change that has nothing to do");
            return;
        }
        self.requests = self.requests.wrapping_add(1);
        let request = self.requests;
        self.notify.insert(
            room_id.clone(),
            NotifyChange::Requested {
                request,
                target: mode,
            },
        );
        self.clear_error(&room_id);
        let name = room.display_name.clone();
        self.publish();

        self.actions.spawn_reporting(&self.events, async move {
            let outcome = ActionOutcome::of(
                port.set_notify(&room_id, mode).await,
                |e| tracing::warn!(%room_id, "failed to change the room's notifications: {e}"),
            );
            let settled = RoomActionEvent::NotifySettled {
                request,
                room_id,
                name,
                outcome,
            };
            AppEvent::RoomAction(settled)
        });
    }

    pub(in crate::app) fn notify_settled(
        &mut self,
        request: u64,
        room_id: RoomId,
        name: &str,
        outcome: ActionOutcome,
        listed: Option<NotifyMode>,
    ) {
        let Some(NotifyChange::Requested {
            request: pending,
            target,
        }) = self.notify.get(&room_id).copied()
        else {
            tracing::debug!(%room_id, "dropping a notification change nobody is waiting for");
            return;
        };
        if pending != request {
            return;
        }
        match outcome {
            ActionOutcome::Done if listed.is_none_or(|mode| mode == target) => {
                self.notify.remove(&room_id);
            }
            ActionOutcome::Done => {
                tracing::debug!(%room_id, "changed, waiting for sync to deliver the push rules");
                self.notify
                    .insert(room_id, NotifyChange::AwaitingEcho { target });
            }
            ActionOutcome::Failed => {
                self.notify.remove(&room_id);
                self.report(
                    &room_id,
                    UserMessage::about(UserMessageKind::NotifyChangeFailed, &name),
                );
            }
        }
        self.publish();
    }

    pub(in crate::app) fn leave(&mut self, port: Arc<dyn LeavePort>, room: Option<&Room>) {
        let Some(room) = room else {
            tracing::debug!("the room is not listed, not leaving it");
            return;
        };
        let room_id = room.id.clone();
        if !self.leaving.insert(room_id.clone()) {
            return;
        }
        self.clear_error(&room_id);
        let name = room.display_name.clone();
        self.publish();

        self.actions.spawn_reporting(&self.events, async move {
            let outcome = ActionOutcome::of(
                port.leave(&room_id).await,
                |e| tracing::warn!(%room_id, "failed to leave the room: {e}"),
            );
            let settled = RoomActionEvent::LeaveSettled {
                room_id,
                name,
                outcome,
            };
            AppEvent::RoomAction(settled)
        });
    }

    pub(in crate::app) fn leave_settled(
        &mut self,
        room_id: &RoomId,
        name: &str,
        outcome: ActionOutcome,
    ) {
        if !self.leaving.remove(room_id) {
            tracing::debug!(%room_id, "dropping a leave nobody is waiting for");
            return;
        }
        match outcome {
            ActionOutcome::Done => {
                let sheet_closed = self.drop_sheet_of(room_id);
                let menu_closed = self.menu.as_ref().is_some_and(|menu| menu.id == *room_id);
                if menu_closed {
                    self.menu = None;
                }
                if sheet_closed || menu_closed {
                    self.publish();
                }
            }
            ActionOutcome::Failed => {
                self.report(
                    room_id,
                    UserMessage::about(UserMessageKind::LeaveRoomFailed, &name),
                );
                self.publish();
            }
        }
    }

    pub(in crate::app) fn mark_read(&mut self, port: Arc<dyn RoomInfoPort>, room: Option<&Room>) {
        let Some(room) = room else {
            tracing::debug!("the room is not listed, not marking it read");
            return;
        };
        let room_id = room.id.clone();
        if !self.marking.insert(room_id.clone()) {
            return;
        }
        let name = room.display_name.clone();
        let receipts = self.receipts;
        self.actions.spawn_reporting(&self.events, async move {
            let outcome = ActionOutcome::of(
                port.mark_read(&room_id, receipts).await,
                |e| tracing::warn!(%room_id, "failed to mark the room as read: {e}"),
            );
            let settled = RoomActionEvent::ReadSettled {
                room_id,
                name,
                outcome,
            };
            AppEvent::RoomAction(settled)
        });
    }

    pub(in crate::app) fn read_settled(
        &mut self,
        room_id: &RoomId,
        name: &str,
        outcome: ActionOutcome,
    ) {
        if !self.marking.remove(room_id) {
            tracing::debug!(%room_id, "dropping a read marker nobody is waiting for");
            return;
        }
        if outcome == ActionOutcome::Failed {
            self.report(
                room_id,
                UserMessage::about(UserMessageKind::MarkReadFailed, &name),
            );
            self.publish();
        }
    }

    pub(in crate::app) fn copy_link(
        &mut self,
        port: Arc<dyn RoomInfoPort>,
        room_id: &RoomId,
        name: Option<String>,
    ) {
        let Some(name) = name else {
            tracing::debug!(%room_id, "neither a listed room nor a joined space, not linking it");
            return;
        };
        let room_id = room_id.clone();
        let request = self.links.issue();
        self.actions.spawn_reporting(&self.events, async move {
            let link = port
                .room_link(&room_id)
                .await
                .inspect_err(|e| tracing::warn!(%room_id, "failed to link the room: {e}"))
                .ok();
            let resolved = RoomActionEvent::LinkResolved {
                request,
                name,
                link,
            };
            AppEvent::RoomAction(resolved)
        });
    }

    pub(in crate::app) fn link_resolved(&mut self, request: i32, name: &str, link: Option<String>) {
        if !self.links.settle(request) {
            tracing::debug!(request, "dropping a room link a later copy replaced");
            return;
        }
        match link {
            Some(url) => self.output.publish(Box::new(move |view| {
                view.room_link = CopiedLink {
                    serial: request,
                    url,
                };
            })),
            None => show_toast(
                self.output.as_ref(),
                Toast::Error(UserMessage::about(UserMessageKind::RoomLinkFailed, &name)),
            ),
        }
    }

    pub(super) fn pending_notify(
        &self,
        room_id: &RoomId,
        listed: NotifyMode,
    ) -> (NotifyMode, bool) {
        let change = self.notify.get(room_id).copied();
        (
            change.map_or(listed, NotifyChange::target),
            matches!(change, Some(NotifyChange::Requested { .. })),
        )
    }
}
