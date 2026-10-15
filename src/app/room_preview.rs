use std::collections::HashMap;
use std::sync::Arc;

use super::event::{AppEvent, PreviewEvent};
use super::input::EventSender;
use super::room_directory::Membership;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{ChildAccess, RoomPreviewSheet, RoomPreviewView, ShownPreview, Toast};
use crate::domain::matrix_link::{MatrixLink, RoomAddress, RoomLink};
use crate::domain::room::RoomId;
use crate::domain::room_preview::{PreviewFailure, PreviewMembership, RoomPreview};
use crate::ports::matrix::RoomPreviewPort;
use crate::ports::output::AppOutputPort;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Entry,
    Link,
}

enum Fetch {
    Idle,
    Loading,
    Loaded(Arc<RoomPreview>),
    Failed(PreviewFailure),
}

struct OpenPreview {
    mode: Mode,
    target: Option<RoomLink>,
    fetch: Fetch,
    access: ChildAccess,
    error: UserMessage,
}

impl OpenPreview {
    fn shows(&self, room: &RoomAddress) -> bool {
        self.target
            .as_ref()
            .is_some_and(|target| target.room == *room)
    }

    fn known_room(&self) -> Option<&RoomPreview> {
        match &self.fetch {
            Fetch::Loaded(preview) => Some(preview),
            Fetch::Idle | Fetch::Loading | Fetch::Failed(_) => None,
        }
    }
}

enum JoinStage {
    Requested,
    AwaitingList(RoomId),
}

pub(super) enum JoinedRoom {
    Joined(RoomId),
    Failed,
}

pub(super) struct RoomPreviews {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    tasks: TaskGroup,
    joins: TaskGroup,
    generation: u64,
    open: Option<OpenPreview>,
    pending: HashMap<RoomAddress, JoinStage>,
    avatars_ready: usize,
    held: Option<MatrixLink>,
}

impl RoomPreviews {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            tasks: TaskGroup::new("room-previews"),
            joins: TaskGroup::new("room-link-joins"),
            generation: 0,
            open: None,
            pending: HashMap::new(),
            avatars_ready: 0,
            held: None,
        }
    }

    pub(super) fn open_entry(&mut self) {
        if self
            .open
            .as_ref()
            .is_some_and(|open| open.mode == Mode::Entry)
        {
            return;
        }
        self.begin(Mode::Entry, None, Fetch::Idle);
        self.publish();
    }

    pub(super) fn refuse_address(&mut self) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let mode = open.mode;
        self.begin(mode, None, Fetch::Idle);
        if let Some(open) = self.open.as_mut() {
            open.error = UserMessage::new(UserMessageKind::RoomAddressInvalid);
        }
        self.publish();
    }

    pub(super) fn show(
        &mut self,
        port: Arc<dyn RoomPreviewPort>,
        link: RoomLink,
        membership: &Membership<'_>,
    ) {
        if let Some(open) = self
            .open
            .as_mut()
            .filter(|open| open.shows(&link.room) && !matches!(open.fetch, Fetch::Failed(_)))
        {
            open.target = Some(link);
            self.publish();
            return;
        }
        let mode = self.open.as_ref().map_or(Mode::Link, |open| open.mode);
        tracing::info!(room = %link.room, event = ?link.event, "previewing a linked room");
        let room = link.room.clone();
        let via = link.via.clone();
        self.begin(mode, Some(link), Fetch::Loading);
        self.rederive(membership);
        self.spawn_preview(port, room, via);
        self.publish();
    }

    pub(super) fn retry(&mut self, port: Arc<dyn RoomPreviewPort>) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|open| matches!(open.fetch, Fetch::Failed(_)))
        else {
            return;
        };
        let Some(target) = open.target.clone() else {
            return;
        };
        open.fetch = Fetch::Loading;
        open.error = UserMessage::default();
        self.tasks.cancel_and_detach();
        self.generation = self.generation.wrapping_add(1);
        self.spawn_preview(port, target.room, target.via);
        self.publish();
    }

    pub(super) fn close(&mut self) {
        if self.open.take().is_none() {
            return;
        }
        self.tasks.cancel_and_detach();
        self.generation = self.generation.wrapping_add(1);
        self.forget_awaited_arrivals();
        self.publish();
    }

    pub(super) fn landed(
        &mut self,
        port: Arc<dyn RoomPreviewPort>,
        generation: u64,
        outcome: Result<Box<RoomPreview>, PreviewFailure>,
        membership: &Membership<'_>,
    ) -> Option<RoomLink> {
        if generation != self.generation {
            tracing::debug!("dropping a room preview for a link nobody is looking at");
            return None;
        }
        let open = self
            .open
            .as_mut()
            .filter(|open| matches!(open.fetch, Fetch::Loading))?;
        let target = open.target.clone()?;
        match outcome {
            Ok(preview) => {
                let resolved = RoomLink {
                    room: RoomAddress::Id(preview.id.clone()),
                    ..target.clone()
                };
                if preview.membership == PreviewMembership::Joined {
                    self.pending
                        .entry(target.room)
                        .or_insert_with(|| JoinStage::AwaitingList(preview.id.clone()));
                }
                let avatar = preview.avatar_mxc.clone();
                open.fetch = Fetch::Loaded(Arc::from(preview));
                self.rederive(membership);
                if let Some(mxc) = avatar {
                    self.spawn_avatar(port, mxc);
                }
                self.publish();
                Some(resolved)
            }
            Err(failure) => {
                open.error = failure_message(failure, &target.room);
                open.fetch = Fetch::Failed(failure);
                self.rederive(membership);
                self.publish();
                None
            }
        }
    }

    pub(super) fn avatars_landed(&mut self, generation: u64, ready: usize) {
        if generation != self.generation || ready == 0 {
            return;
        }
        self.avatars_ready = self.avatars_ready.wrapping_add(ready);
        self.publish();
    }

    pub(super) fn join(
        &mut self,
        port: Arc<dyn RoomPreviewPort>,
        address: &str,
        membership: &Membership<'_>,
    ) {
        let Some(open) = self.open.as_mut() else {
            tracing::debug!(address, "no room preview is open, not joining");
            return;
        };
        let Some(target) = open
            .target
            .clone()
            .filter(|target| target.room.as_str() == address)
        else {
            tracing::debug!(address, "the room preview shows another room, not joining");
            return;
        };
        if open.access != ChildAccess::Join || self.pending.contains_key(&target.room) {
            tracing::debug!(address, "the previewed room cannot be joined now");
            return;
        }
        let known = open.known_room().map(|preview| preview.id.clone());
        let name = open
            .known_room()
            .map_or_else(|| address.to_owned(), |preview| preview.name.clone());
        open.error = UserMessage::default();
        self.pending
            .insert(target.room.clone(), JoinStage::Requested);
        self.rederive(membership);
        self.publish();

        let events = self.events.clone();
        let cancel = self.joins.token();
        self.joins.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                joined = port.join(&target.room, known.as_ref(), &target.via) => match joined {
                    Ok(room_id) => JoinedRoom::Joined(room_id),
                    Err(e) => {
                        tracing::warn!(room = %target.room, "failed to join a linked room: {e}");
                        JoinedRoom::Failed
                    }
                },
            };
            drop(events.send(AppEvent::Preview(PreviewEvent::JoinSettled {
                room: target.room,
                name,
                outcome,
            })));
        });
    }

    pub(super) fn join_settled(
        &mut self,
        room: &RoomAddress,
        name: &str,
        outcome: JoinedRoom,
        membership: &Membership<'_>,
    ) -> Option<RoomLink> {
        if !matches!(self.pending.get(room), Some(JoinStage::Requested)) {
            tracing::debug!(%room, "dropping a join nobody is waiting for");
            return None;
        }
        self.pending.remove(room);
        let shown = self.open.as_ref().is_some_and(|open| open.shows(room));
        match outcome {
            JoinedRoom::Joined(room_id) if shown => self.await_list(room, room_id, membership),
            JoinedRoom::Joined(room_id) => {
                tracing::debug!(%room_id, "joined a linked room whose preview has closed");
                None
            }
            JoinedRoom::Failed => {
                self.report(
                    room,
                    UserMessage::about(UserMessageKind::JoinRoomFailed, &name),
                );
                self.rederive(membership);
                self.publish();
                None
            }
        }
    }

    fn await_list(
        &mut self,
        room: &RoomAddress,
        room_id: RoomId,
        membership: &Membership<'_>,
    ) -> Option<RoomLink> {
        if membership.has_joined(&room_id) {
            return self.arrive(&room_id);
        }
        tracing::debug!(%room_id, "joined, waiting for sync to list the room");
        self.pending
            .insert(room.clone(), JoinStage::AwaitingList(room_id));
        self.rederive(membership);
        self.publish();
        None
    }

    pub(super) fn follow(&mut self, membership: &Membership<'_>) -> Option<RoomLink> {
        let room = self.open.as_ref()?.target.as_ref()?.room.clone();
        if let Some(JoinStage::AwaitingList(room_id)) = self.pending.get(&room)
            && membership.has_joined(room_id)
        {
            let room_id = room_id.clone();
            self.pending.remove(&room);
            return self.arrive(&room_id);
        }
        if self.rederive(membership) {
            self.publish();
        }
        None
    }

    pub(super) fn hold(&mut self, link: MatrixLink) {
        tracing::info!("holding a link until the session lists its rooms");
        self.held = Some(link);
    }

    pub(super) fn take_held(&mut self) -> Option<MatrixLink> {
        self.held.take()
    }

    pub(super) async fn restart(&mut self) {
        self.tasks.restart().await;
        self.joins.restart().await;
        self.open = None;
        self.pending.clear();
        self.held = None;
        self.avatars_ready = 0;
    }

    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
        self.joins.shutdown().await;
    }

    fn begin(&mut self, mode: Mode, target: Option<RoomLink>, fetch: Fetch) {
        self.tasks.cancel_and_detach();
        self.generation = self.generation.wrapping_add(1);
        self.forget_awaited_arrivals();
        self.open = Some(OpenPreview {
            mode,
            target,
            fetch,
            access: ChildAccess::Join,
            error: UserMessage::default(),
        });
    }

    fn arrive(&mut self, room_id: &RoomId) -> Option<RoomLink> {
        let target = self.open.take()?.target?;
        self.tasks.cancel_and_detach();
        self.generation = self.generation.wrapping_add(1);
        self.publish();
        Some(RoomLink {
            room: RoomAddress::Id(room_id.clone()),
            ..target
        })
    }

    fn spawn_preview(
        &mut self,
        port: Arc<dyn RoomPreviewPort>,
        room: RoomAddress,
        via: Vec<String>,
    ) {
        let generation = self.generation;
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                previewed = port.preview(&room, &via) => previewed,
            };
            drop(events.send(AppEvent::Preview(PreviewEvent::Landed {
                generation,
                outcome: outcome.map(Box::new),
            })));
        });
    }

    fn spawn_avatar(&mut self, port: Arc<dyn RoomPreviewPort>, mxc: String) {
        let generation = self.generation;
        let events = self.events.clone();
        let cancel = self.tasks.token();
        self.tasks.spawn(async move {
            let mxcs = [mxc];
            let ready = tokio::select! {
                () = cancel.cancelled() => return,
                ready = port.fetch_avatars(&mxcs) => ready,
            };
            drop(events.send(AppEvent::Preview(PreviewEvent::AvatarsReady {
                generation,
                ready,
            })));
        });
    }

    fn rederive(&mut self, membership: &Membership<'_>) -> bool {
        let Some(open) = self.open.as_mut() else {
            return false;
        };
        let pending = open
            .target
            .as_ref()
            .is_some_and(|target| self.pending.contains_key(&target.room));
        let access = if pending {
            ChildAccess::Joining
        } else {
            open.known_room()
                .map_or(ChildAccess::Join, |preview| access(preview, membership))
        };
        if open.access == access {
            return false;
        }
        open.access = access;
        true
    }

    fn forget_awaited_arrivals(&mut self) {
        self.pending
            .retain(|_, stage| matches!(stage, JoinStage::Requested));
    }

    fn report(&mut self, room: &RoomAddress, message: UserMessage) {
        match self.open.as_mut().filter(|open| open.shows(room)) {
            Some(open) => open.error = message,
            None => show_toast(self.output.as_ref(), Toast::Error(message)),
        }
    }

    fn publish(&self) {
        let view = self.view();
        self.output
            .publish(Box::new(move |state| state.room_preview = view));
    }

    fn view(&self) -> RoomPreviewView {
        let Some(open) = self.open.as_ref() else {
            return RoomPreviewView::default();
        };
        let sheet = match &open.fetch {
            Fetch::Idle => RoomPreviewSheet::Idle,
            Fetch::Loading => RoomPreviewSheet::Loading,
            Fetch::Loaded(preview) => RoomPreviewSheet::Loaded(ShownPreview {
                room: Arc::clone(preview),
                opens_event: open
                    .target
                    .as_ref()
                    .is_some_and(|target| target.event.is_some()),
            }),
            Fetch::Failed(PreviewFailure::NoSuchRoom) => RoomPreviewSheet::Missing,
            Fetch::Failed(PreviewFailure::Unavailable) => RoomPreviewSheet::Unavailable,
            Fetch::Failed(PreviewFailure::Failed) => RoomPreviewSheet::Failed,
        };
        RoomPreviewView {
            sheet,
            address: open
                .target
                .as_ref()
                .map(|target| target.room.to_string())
                .unwrap_or_default(),
            entry: open.mode == Mode::Entry,
            access: open.access,
            error: open.error.clone(),
            avatars_ready: self.avatars_ready,
        }
    }
}

fn failure_message(failure: PreviewFailure, room: &RoomAddress) -> UserMessage {
    match failure {
        PreviewFailure::NoSuchRoom => UserMessage::about(UserMessageKind::RoomNotFound, room),
        PreviewFailure::Failed => UserMessage::about(UserMessageKind::RoomPreviewFailed, room),
        PreviewFailure::Unavailable => UserMessage::default(),
    }
}

fn access(preview: &RoomPreview, membership: &Membership<'_>) -> ChildAccess {
    match preview.membership {
        PreviewMembership::Joined => ChildAccess::Joining,
        PreviewMembership::Invited => ChildAccess::Join,
        PreviewMembership::Knocked => ChildAccess::Knock,
        PreviewMembership::Banned => ChildAccess::Unavailable,
        PreviewMembership::Outsider => preview
            .join_rule
            .as_ref()
            .map_or(ChildAccess::Join, |rule| membership.join_access(rule)),
    }
}
