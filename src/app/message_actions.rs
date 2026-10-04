use std::collections::HashSet;
use std::sync::Arc;

use super::active_timeline::report_delete_failure;
use super::event::{AppEvent, MessageActionEvent};
use super::input::EventSender;
use super::room_info::ActionOutcome;
use super::send_lanes::SendLanes;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{MessageLink, Toast};
use crate::domain::room::RoomId;
use crate::ports::matrix::{RoomInfoPort, TimelinePort};
use crate::ports::output::AppOutputPort;

pub(super) struct MessageActions {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    links: i32,
    awaited_link: Option<i32>,
    deleting: HashSet<String>,
}

impl MessageActions {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            links: 0,
            awaited_link: None,
            deleting: HashSet::new(),
        }
    }

    pub(super) fn delete(
        &mut self,
        lanes: &mut SendLanes,
        port: Arc<dyn TimelinePort>,
        room_id: RoomId,
        event_id: String,
    ) {
        if !self.deleting.insert(event_id.clone()) {
            tracing::debug!(event_id, "ignoring a deletion already on its way");
            return;
        }
        let events = self.events.clone();
        lanes.spawn(room_id.clone(), async move {
            let outcome = match port.delete_message(&room_id, &event_id).await {
                Ok(()) => ActionOutcome::Done,
                Err(e) => {
                    tracing::warn!(%room_id, event_id, "the deletion was refused: {e}");
                    ActionOutcome::Failed
                }
            };
            let settled = MessageActionEvent::DeletionSettled { event_id, outcome };
            drop(events.send(AppEvent::MessageAction(settled)));
        });
    }

    pub(super) fn deletion_settled(&mut self, event_id: &str, outcome: ActionOutcome) {
        if !self.deleting.remove(event_id) {
            return;
        }
        if outcome == ActionOutcome::Failed {
            report_delete_failure(self.output.as_ref());
        }
    }

    pub(super) fn copy_link(
        &mut self,
        group: &mut TaskGroup,
        port: Arc<dyn RoomInfoPort>,
        room_id: RoomId,
        event_id: String,
    ) {
        self.links = self.links.wrapping_add(1);
        let request = self.links;
        self.awaited_link = Some(request);
        let events = self.events.clone();
        let cancel = group.token();
        group.spawn(async move {
            let link = tokio::select! {
                () = cancel.cancelled() => return,
                link = port.event_link(&room_id, &event_id) => link
                    .inspect_err(|e| tracing::warn!(%room_id, event_id, "failed to link a message: {e}"))
                    .ok(),
            };
            let resolved = MessageActionEvent::LinkResolved { request, link };
            drop(events.send(AppEvent::MessageAction(resolved)));
        });
    }

    pub(super) fn link_resolved(&mut self, request: i32, link: Option<String>) {
        if self.awaited_link != Some(request) {
            tracing::debug!(request, "dropping a message link a later copy replaced");
            return;
        }
        self.awaited_link = None;
        match link {
            Some(url) => self.output.publish(Box::new(move |view| {
                view.message_link = MessageLink {
                    serial: request,
                    url,
                };
            })),
            None => show_toast(
                self.output.as_ref(),
                Toast::Error(UserMessage::new(UserMessageKind::MessageLinkFailed)),
            ),
        }
    }

    pub(super) fn forget(&mut self) {
        self.awaited_link = None;
        self.deleting.clear();
    }
}
