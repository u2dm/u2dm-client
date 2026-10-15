use std::collections::HashMap;
use std::sync::Arc;

use super::event::{AppEvent, CreationSettled};
use super::input::EventSender;
use super::room_directory::Membership;
use super::show_toast;
use super::space_index::ListedSpace;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{CreateRoomStatus, CreateRoomView, Toast};
use crate::domain::room::RoomId;
use crate::domain::room_creation::{
    ChildLink, CreateFailure, DraftProblem, JoinAccess, RoomBlueprint, RoomDraft, RoomKind,
};
use crate::ports::matrix::RoomCreationPort;
use crate::ports::output::AppOutputPort;

struct OpenForm {
    kind: RoomKind,
    parent: Option<ListedSpace>,
    server: String,
    creation: Option<u64>,
    error: UserMessage,
}

struct Labels {
    name: String,
    kind: RoomKind,
    address: String,
    parent: Option<ListedSpace>,
}

struct Awaited {
    room_id: RoomId,
    kind: RoomKind,
    parent: Option<RoomId>,
}

enum Stage {
    Requested(Labels),
    AwaitingList(Awaited),
}

pub(super) struct Created {
    pub(super) room_id: RoomId,
    pub(super) kind: RoomKind,
}

pub(super) struct RoomCreation {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    creations: TaskGroup,
    next: u64,
    open: Option<OpenForm>,
    pending: HashMap<u64, Stage>,
}

impl RoomCreation {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            creations: TaskGroup::new("room-creations"),
            next: 0,
            open: None,
            pending: HashMap::new(),
        }
    }

    pub(super) fn open(&mut self, kind: RoomKind, parent: Option<ListedSpace>, server: String) {
        if self.open.is_some() {
            tracing::debug!("the create dialog is already open");
            return;
        }
        tracing::debug!(
            ?kind,
            in_space = parent.is_some(),
            "opening the create dialog"
        );
        self.open = Some(OpenForm {
            kind,
            parent,
            server,
            creation: None,
            error: UserMessage::default(),
        });
        self.publish();
    }

    pub(super) fn close(&mut self) {
        if self.open.take().is_none() {
            return;
        }
        self.forget_awaited_arrivals();
        self.publish();
    }

    pub(super) fn create(&mut self, port: Arc<dyn RoomCreationPort>, draft: &RoomDraft) {
        let Some(open) = self.open.as_mut() else {
            tracing::debug!("the create dialog is closed, not creating");
            return;
        };
        if open.creation.is_some() {
            tracing::debug!("the create dialog is already creating");
            return;
        }
        let offered = open.parent.as_ref().map(|parent| &parent.id);
        let blueprint = match RoomBlueprint::new(draft, offered, &open.server) {
            Ok(blueprint) => blueprint,
            Err(problem) => {
                open.error = problem_message(problem, draft);
                self.publish();
                return;
            }
        };
        let creation = self.next;
        self.next = self.next.wrapping_add(1);
        open.creation = Some(creation);
        open.error = UserMessage::default();
        let labels = Labels {
            name: blueprint.name().to_owned(),
            kind: blueprint.kind(),
            address: match blueprint.access() {
                JoinAccess::Public(localpart) => localpart.alias_on(&open.server),
                JoinAccess::InviteOnly | JoinAccess::SpaceMembers(_) => String::new(),
            },
            parent: blueprint.parent().and(open.parent.clone()),
        };
        self.pending.insert(creation, Stage::Requested(labels));
        self.publish();

        let events = self.events.clone();
        let cancel = self.creations.token();
        self.creations.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                outcome = port.create_room(&blueprint) => outcome,
            };
            drop(events.send(AppEvent::RoomCreation(CreationSettled {
                creation,
                outcome,
            })));
        });
    }

    pub(super) fn settled(
        &mut self,
        settled: CreationSettled,
        membership: &Membership<'_>,
    ) -> Option<Created> {
        let CreationSettled { creation, outcome } = settled;
        let Some(Stage::Requested(labels)) = self.pending.remove(&creation) else {
            tracing::debug!(creation, "dropping a creation nobody is waiting for");
            return None;
        };
        let shown = self
            .open
            .as_ref()
            .is_some_and(|open| open.creation == Some(creation));
        let created = match outcome {
            Ok(created) => created,
            Err(failure) => {
                self.report_failure(shown, failure_message(failure, &labels));
                return None;
            }
        };
        if created.child == ChildLink::Failed
            && let Some(parent) = &labels.parent
        {
            show_toast(
                self.output.as_ref(),
                Toast::Error(UserMessage::about(
                    UserMessageKind::AddToSpaceFailed,
                    &parent.name,
                )),
            );
        }
        if !shown {
            tracing::info!(room_id = %created.room_id, "created a room after its dialog closed");
            return None;
        }
        let awaited = Awaited {
            room_id: created.room_id,
            kind: labels.kind,
            parent: labels
                .parent
                .filter(|_| created.child == ChildLink::Added)
                .map(|parent| parent.id),
        };
        if arrived(&awaited, membership) {
            return Some(self.arrive(awaited));
        }
        tracing::debug!(room_id = %awaited.room_id, "created, waiting for sync to list the room");
        self.pending.insert(creation, Stage::AwaitingList(awaited));
        None
    }

    pub(super) fn follow(&mut self, membership: &Membership<'_>) -> Option<Created> {
        let creation = self.open.as_ref()?.creation?;
        let Some(Stage::AwaitingList(awaited)) = self.pending.get(&creation) else {
            return None;
        };
        if !arrived(awaited, membership) {
            return None;
        }
        let Some(Stage::AwaitingList(awaited)) = self.pending.remove(&creation) else {
            return None;
        };
        Some(self.arrive(awaited))
    }

    pub(super) async fn restart(&mut self) {
        self.creations.restart().await;
        self.open = None;
        self.pending.clear();
    }

    pub(super) async fn shutdown(&mut self) {
        self.creations.shutdown().await;
    }

    fn arrive(&mut self, awaited: Awaited) -> Created {
        tracing::info!(room_id = %awaited.room_id, "the created room is listed, opening it");
        self.open = None;
        self.publish();
        Created {
            room_id: awaited.room_id,
            kind: awaited.kind,
        }
    }

    fn report_failure(&mut self, shown: bool, message: UserMessage) {
        match self.open.as_mut().filter(|_| shown) {
            Some(open) => {
                open.creation = None;
                open.error = message;
                self.publish();
            }
            None => show_toast(self.output.as_ref(), Toast::Error(message)),
        }
    }

    fn forget_awaited_arrivals(&mut self) {
        self.pending
            .retain(|_, stage| matches!(stage, Stage::Requested(_)));
    }

    fn publish(&self) {
        let view = self.view();
        self.output
            .publish(Box::new(move |state| state.create_room = view));
    }

    fn view(&self) -> CreateRoomView {
        let Some(open) = self.open.as_ref() else {
            return CreateRoomView::default();
        };
        CreateRoomView {
            status: if open.creation.is_some() {
                CreateRoomStatus::Creating
            } else {
                CreateRoomStatus::Editing
            },
            kind: open.kind,
            parent_name: open
                .parent
                .as_ref()
                .map(|parent| parent.name.clone())
                .unwrap_or_default(),
            server_name: open.server.clone(),
            error: open.error.clone(),
        }
    }
}

fn arrived(awaited: &Awaited, membership: &Membership<'_>) -> bool {
    let listed = match awaited.kind {
        RoomKind::Room => membership.has_room(&awaited.room_id),
        RoomKind::Space => membership.has_space(&awaited.room_id),
    };
    listed
        && awaited
            .parent
            .as_ref()
            .is_none_or(|parent| membership.lists_child(parent, &awaited.room_id))
}

fn problem_message(problem: DraftProblem, draft: &RoomDraft) -> UserMessage {
    match problem {
        DraftProblem::NameMissing => UserMessage::new(UserMessageKind::RoomNameMissing),
        DraftProblem::AddressMissing => UserMessage::new(UserMessageKind::RoomAddressMissing),
        DraftProblem::AddressUnusable => UserMessage::new(UserMessageKind::RoomAddressUnusable),
        DraftProblem::SpaceMissing => {
            UserMessage::about(UserMessageKind::RoomCreateFailed, &draft.name.trim())
        }
    }
}

fn failure_message(failure: CreateFailure, labels: &Labels) -> UserMessage {
    match failure {
        CreateFailure::AddressTaken => {
            UserMessage::about(UserMessageKind::RoomAddressTaken, &labels.address)
        }
        CreateFailure::Refused => UserMessage::new(UserMessageKind::RoomCreateRefused),
        CreateFailure::Unsupported => UserMessage::new(UserMessageKind::RoomCreateUnsupported),
        CreateFailure::RateLimited => UserMessage::new(UserMessageKind::RateLimited),
        CreateFailure::Failed => {
            UserMessage::about(UserMessageKind::RoomCreateFailed, &labels.name)
        }
    }
}
