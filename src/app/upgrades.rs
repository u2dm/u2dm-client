use std::collections::HashMap;
use std::sync::Arc;

use super::event::AppEvent;
use super::input::EventSender;
use super::room_directory::Membership;
use super::room_info::ActionOutcome;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{Toast, UpgradeLink, UpgradeView};
use crate::domain::room::{Room, RoomId};
use crate::error::Result as AppResult;
use crate::ports::matrix::UpgradePort;
use crate::ports::output::AppOutputPort;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Toward {
    Successor,
    Predecessor,
}

impl Toward {
    fn target(self, room: &Room) -> Option<&RoomId> {
        match self {
            Self::Successor => room.successor.as_ref(),
            Self::Predecessor => room.predecessor.as_ref(),
        }
    }

    async fn join(self, port: &dyn UpgradePort, room_id: &RoomId) -> AppResult<()> {
        match self {
            Self::Successor => port.join_successor(room_id).await,
            Self::Predecessor => port.join_predecessor(room_id).await,
        }
    }

    fn failure(self) -> UserMessageKind {
        match self {
            Self::Successor => UserMessageKind::JoinNewRoomFailed,
            Self::Predecessor => UserMessageKind::JoinOldRoomFailed,
        }
    }
}

pub(super) struct UpgradeJoined {
    pub(super) from: RoomId,
    pub(super) target: RoomId,
    pub(super) toward: Toward,
    pub(super) name: String,
    pub(super) outcome: ActionOutcome,
}

#[derive(Debug)]
enum JoinSkipped {
    RoomUnlisted,
    NoUpgradeLink,
    AlreadyJoined,
    AlreadyJoining,
}

enum Stage {
    Joining,
    AwaitingList,
}

struct PendingJoin {
    from: RoomId,
    stage: Stage,
}

pub(super) struct RoomUpgrades {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    joins: TaskGroup,
    pending: HashMap<RoomId, PendingJoin>,
    shown: UpgradeView,
}

impl RoomUpgrades {
    pub(super) fn new(output: Arc<dyn AppOutputPort>, events: EventSender) -> Self {
        Self {
            output,
            events,
            joins: TaskGroup::new("upgrade-joins"),
            pending: HashMap::new(),
            shown: UpgradeView::default(),
        }
    }

    pub(super) fn join(
        &mut self,
        port: Arc<dyn UpgradePort>,
        toward: Toward,
        room: Option<&Room>,
        membership: &Membership<'_>,
    ) {
        let (room, target) = match self.target_to_join(toward, room, membership) {
            Ok(found) => found,
            Err(skip) => {
                tracing::debug!(?toward, ?skip, "not joining across the room upgrade");
                return;
            }
        };
        self.pending.insert(
            target.clone(),
            PendingJoin {
                from: room.id.clone(),
                stage: Stage::Joining,
            },
        );
        self.spawn_join(port, toward, room, target);
    }

    fn target_to_join<'a>(
        &self,
        toward: Toward,
        room: Option<&'a Room>,
        membership: &Membership<'_>,
    ) -> Result<(&'a Room, RoomId), JoinSkipped> {
        let room = room.ok_or(JoinSkipped::RoomUnlisted)?;
        let target = toward
            .target(room)
            .cloned()
            .ok_or(JoinSkipped::NoUpgradeLink)?;
        if membership.has_room(&target) {
            return Err(JoinSkipped::AlreadyJoined);
        }
        if self.pending.contains_key(&target) {
            return Err(JoinSkipped::AlreadyJoining);
        }
        Ok((room, target))
    }

    fn spawn_join(
        &mut self,
        port: Arc<dyn UpgradePort>,
        toward: Toward,
        room: &Room,
        target: RoomId,
    ) {
        let from = room.id.clone();
        let name = room.display_name.clone();
        let events = self.events.clone();
        let cancel = self.joins.token();
        self.joins.spawn(async move {
            let outcome = tokio::select! {
                () = cancel.cancelled() => return,
                joined = toward.join(port.as_ref(), &from) => match joined {
                    Ok(()) => ActionOutcome::Done,
                    Err(e) => {
                        tracing::warn!(
                            %from,
                            %target,
                            ?toward,
                            "failed to join the other end of a room upgrade: {e}"
                        );
                        ActionOutcome::Failed
                    }
                },
            };
            drop(events.send(AppEvent::UpgradeJoinSettled(UpgradeJoined {
                from,
                target,
                toward,
                name,
                outcome,
            })));
        });
    }

    pub(super) fn settled(
        &mut self,
        joined: UpgradeJoined,
        membership: &Membership<'_>,
        selected: Option<&RoomId>,
    ) -> Option<RoomId> {
        let UpgradeJoined {
            from,
            target,
            toward,
            name,
            outcome,
        } = joined;
        if !self.is_joining(&target) {
            tracing::debug!(%target, "dropping a join result nobody is waiting for");
            return None;
        }
        self.pending.remove(&target);
        if matches!(outcome, ActionOutcome::Failed) {
            show_toast(
                self.output.as_ref(),
                Toast::Error(UserMessage::about(toward.failure(), &name)),
            );
            return None;
        }
        if selected != Some(&from) {
            tracing::debug!(%target, "joined a room whose link the user has moved away from");
            return None;
        }
        if membership.has_room(&target) {
            return Some(target);
        }
        tracing::debug!(%target, "joined, waiting for sync to list the room");
        self.pending.insert(
            target,
            PendingJoin {
                from,
                stage: Stage::AwaitingList,
            },
        );
        None
    }

    fn is_joining(&self, target: &RoomId) -> bool {
        matches!(
            self.pending.get(target),
            Some(PendingJoin {
                stage: Stage::Joining,
                ..
            })
        )
    }

    pub(super) fn follow(
        &mut self,
        membership: &Membership<'_>,
        selected: Option<&RoomId>,
    ) -> Option<RoomId> {
        let mut arrival = None;
        self.pending.retain(|target, join| {
            let listed = matches!(join.stage, Stage::AwaitingList) && membership.has_room(target);
            if listed && selected == Some(&join.from) {
                arrival = Some(target.clone());
            }
            !listed
        });
        arrival
    }

    pub(super) fn retarget(&mut self, selected: &RoomId) {
        self.pending
            .retain(|_, join| matches!(join.stage, Stage::Joining) || join.from == *selected);
    }

    pub(super) fn refresh(&mut self, selected: Option<&Room>, membership: &Membership<'_>) {
        let view = selected.map_or_else(UpgradeView::default, |room| UpgradeView {
            room_id: Some(room.id.clone()),
            successor: self.link(room.successor.as_ref(), membership),
            predecessor: self.link(room.predecessor.as_ref(), membership),
        });
        if view == self.shown {
            return;
        }
        self.shown = view.clone();
        self.output
            .publish(Box::new(move |state| state.upgrade = view));
    }

    fn link(&self, target: Option<&RoomId>, membership: &Membership<'_>) -> UpgradeLink {
        let Some(target) = target.cloned() else {
            return UpgradeLink::Hidden;
        };
        if self.pending.contains_key(&target) {
            UpgradeLink::Joining(target)
        } else if membership.has_room(&target) {
            UpgradeLink::Open(target)
        } else {
            UpgradeLink::Join(target)
        }
    }

    pub(super) async fn restart(&mut self) {
        self.joins.restart().await;
        self.pending.clear();
        self.shown = UpgradeView::default();
    }

    pub(super) async fn shutdown(&mut self) {
        self.joins.shutdown().await;
    }
}
