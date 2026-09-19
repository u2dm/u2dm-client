use std::sync::Arc;

use tokio::sync::mpsc;

use super::event::{AppEvent, TimelineEvent};
use super::input::EventSender;
use super::task_group::TaskGroup;
use crate::commands::effects::Effect;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::Toast;
use crate::domain::room::RoomId;
use crate::domain::timeline::{
    FailedSend,
    AudioLookup, JumpTarget, PaginationDirection, PaginationOutcome, ScrollMode, TimelineAdvance,
    TimelineCommand, TimelineFocus, TimelinePatch, TimelineStatus, TimelineUpdate,
};
use crate::domain::viewport::ViewportController;
use crate::ports::matrix::TimelinePort;
use crate::ports::output::AppOutputPort;

const TIMELINE_CHANNEL_CAP: usize = 256;

pub(super) struct ActiveTimeline {
    events: EventSender,
    output: Arc<dyn AppOutputPort>,
    tasks: TaskGroup,
    viewport: ViewportController,
    timeline_cmd_tx: Option<mpsc::UnboundedSender<TimelineCommand>>,
    active_room_id: Option<RoomId>,
    generation: i32,
    at_bottom: bool,
    new_messages: u32,
    live: bool,
}

impl ActiveTimeline {
    pub(super) fn new(events: EventSender, output: Arc<dyn AppOutputPort>) -> Self {
        Self {
            events,
            output,
            tasks: TaskGroup::new("timeline"),
            viewport: ViewportController::new(),
            timeline_cmd_tx: None,
            active_room_id: None,
            generation: 0,
            at_bottom: true,
            new_messages: 0,
            live: true,
        }
    }

    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
        self.reset_state();
    }

    pub(super) fn is_live(&self) -> bool {
        self.live
    }

    pub(super) async fn select_room(
        &mut self,
        timeline: Arc<dyn TimelinePort>,
        room_id: RoomId,
        generation: i32,
        focus: TimelineFocus,
    ) {
        tracing::info!(%room_id, generation, ?focus, "opening timeline");
        self.tasks.cancel_and_detach();

        let live = focus.is_live();
        self.viewport = ViewportController::new();
        self.active_room_id = Some(room_id.clone());
        self.generation = generation;
        self.at_bottom = true;
        self.new_messages = 0;
        self.live = live;
        self.emit_pagination_state();

        self.emit_reset(room_id.clone(), generation, live).await;

        let (tl_tx, mut tl_rx) = mpsc::channel::<TimelineUpdate>(TIMELINE_CHANNEL_CAP);
        let (tl_cmd_tx, tl_cmd_rx) = mpsc::unbounded_channel::<TimelineCommand>();
        self.timeline_cmd_tx = Some(tl_cmd_tx);

        let output = Arc::clone(&self.output);
        let events = self.events.clone();
        let token = self.tasks.token();
        let rid = room_id.clone();

        let mut forwarder = Forwarder {
            output: Arc::clone(&output),
            events,
            room_id: rid.clone(),
            generation,
            live,
            next_snapshot: Snapshot::Opening,
        };

        self.tasks.spawn(async move {
            let subscribe = timeline.subscribe_timeline(&room_id, focus, tl_tx, tl_cmd_rx);
            let forward = forwarder.run(&mut tl_rx);

            tokio::select! {
                result = subscribe => {
                    if let Err(e) = result {
                        tracing::warn!("timeline subscription failed: {e}");
                        output.emit(Effect::TimelineStatus {
                            room_id: rid.clone(),
                            generation,
                            status: TimelineStatus::Failed { retryable: true },
                        }).await;
                    } else {
                        tracing::debug!("timeline subscription ended");
                        output
                            .emit(Effect::TimelineStatus {
                                room_id: rid.clone(),
                                generation,
                                status: TimelineStatus::Disconnected,
                            })
                            .await;
                    }
                }
                () = forward => {
                    tracing::debug!("timeline forwarder stopped");
                }
                () = token.cancelled() => {
                    tracing::debug!("timeline subscription cancelled");
                }
            }
        });
    }

    pub(super) async fn clear_room(&mut self, generation: i32) {
        tracing::info!(generation, "clearing active room");
        self.tasks.cancel_and_detach();
        self.reset_state();
        self.generation = generation;
        self.emit_pagination_state();

        self.output
            .emit(Effect::Timeline {
                room_id: RoomId::new(String::new()),
                generation,
                patch: Box::new(TimelinePatch::Clear),
            })
            .await;
    }

    pub(super) fn spawn_resolve_failed_send(
        &self,
        group: &mut TaskGroup,
        timeline: Arc<dyn TimelinePort>,
        room_id: RoomId,
        local_id: String,
        action: FailedSend,
    ) {
        let output = Arc::clone(&self.output);
        group.spawn(async move {
            let result = match action {
                FailedSend::Retry => timeline.resend(&room_id, &local_id).await,
                FailedSend::Discard => timeline.discard_send(&room_id, &local_id).await,
            };
            if let Err(e) = result {
                tracing::warn!("failed to resolve a wedged send: {e}");
                super::show_toast(
                    output.as_ref(),
                    Toast::Error(UserMessage::new(UserMessageKind::SendMessageFailed)),
                );
            }
        });
    }

    pub(super) fn room_id(&self) -> Option<&RoomId> {
        self.active_room_id.as_ref()
    }

    pub(super) fn is_current(&self, room_id: &RoomId, generation: i32) -> bool {
        self.generation == generation && self.active_room_id.as_ref() == Some(room_id)
    }

    pub(super) fn paginate_backwards(&mut self, room_id: &RoomId, generation: i32) {
        if !self.is_current(room_id, generation) {
            return;
        }
        if !self.viewport.should_paginate_backwards() {
            return;
        }
        let Some(tx) = &self.timeline_cmd_tx else {
            return;
        };
        self.viewport.set_backwards_loading(true);
        if tx.send(TimelineCommand::PaginateBackwards).is_err() {
            tracing::debug!("timeline command channel closed");
            self.viewport.set_backwards_loading(false);
        }
        self.emit_pagination_state();
    }

    pub(super) fn paginate_forwards(&mut self, room_id: &RoomId, generation: i32) {
        if !self.is_current(room_id, generation) {
            return;
        }
        if !self.viewport.should_paginate_forwards() {
            return;
        }
        let Some(tx) = &self.timeline_cmd_tx else {
            return;
        };
        self.viewport.set_forwards_loading(true);
        if tx.send(TimelineCommand::PaginateForwards).is_err() {
            tracing::debug!("timeline command channel closed");
            self.viewport.set_forwards_loading(false);
        }
        self.emit_pagination_state();
    }

    pub(super) fn complete_pagination(
        &mut self,
        room_id: &RoomId,
        generation: i32,
        direction: PaginationDirection,
        outcome: PaginationOutcome,
    ) {
        if !self.is_current(room_id, generation) {
            return;
        }

        let hit_end = match outcome {
            PaginationOutcome::Completed { hit_end } => {
                self.viewport.complete_pagination(direction, hit_end);
                hit_end
            }
            PaginationOutcome::Failed => {
                self.viewport.fail_pagination(direction);
                super::show_toast(
                    self.output.as_ref(),
                    Toast::Error(UserMessage::new(UserMessageKind::LoadMoreFailed)),
                );
                false
            }
        };
        self.emit_pagination_state();

        if !matches!(direction, PaginationDirection::Forwards) || !hit_end {
            return;
        }

        let caught_up_with_live = !self.live;
        if caught_up_with_live {
            self.refocus(room_id, generation, TimelineFocus::Live);
            return;
        }

        if self.at_bottom {
            self.clear_new_messages(generation);
        }
    }

    pub(super) fn settle_read_position(
        &mut self,
        room_id: &RoomId,
        generation: i32,
        advance: TimelineAdvance,
    ) {
        if !self.is_current(room_id, generation) {
            return;
        }
        match advance {
            TimelineAdvance::Focused => self.at_bottom = false,
            TimelineAdvance::Anchored { count } => {
                self.at_bottom = false;
                self.add_new_messages(generation, count);
            }
            TimelineAdvance::Appended {
                total,
                from_others,
                opens_room,
            } => {
                if self.at_bottom {
                    if opens_room || from_others {
                        self.mark_read();
                    }
                } else if total > 0 {
                    self.add_new_messages(generation, total);
                }
            }
        }
    }

    fn add_new_messages(&mut self, generation: i32, count: u32) {
        self.new_messages = self.new_messages.saturating_add(count);
        self.emit_new_messages(generation, self.new_messages);
    }

    fn clear_new_messages(&mut self, generation: i32) {
        self.new_messages = 0;
        self.emit_new_messages(generation, 0);
    }

    fn refocus(&self, room_id: &RoomId, generation: i32, focus: TimelineFocus) {
        let refocus = TimelineEvent::Refocus {
            room_id: room_id.clone(),
            generation,
            focus,
        };
        drop(self.events.send(AppEvent::Timeline(refocus)));
    }

    pub(super) fn jump_to_event(&mut self, event_id: String) {
        let Some(tx) = &self.timeline_cmd_tx else {
            return;
        };
        if tx.send(TimelineCommand::JumpTo(event_id)).is_err() {
            tracing::debug!("timeline command channel closed");
        }
    }

    pub(super) fn locate_audio(&self, request: u64, lookup: AudioLookup) -> Option<RoomId> {
        let room_id = self.active_room_id.clone()?;
        let tx = self.timeline_cmd_tx.as_ref()?;
        tx.send(TimelineCommand::LocateAudio { request, lookup })
            .ok()
            .map(|()| room_id)
    }

    pub(super) fn is_active_room(&self, room_id: &RoomId) -> bool {
        self.active_room_id.as_ref() == Some(room_id)
    }

    pub(super) fn toggle_reaction(&mut self, event_id: String, key: String) {
        let Some(tx) = &self.timeline_cmd_tx else {
            return;
        };
        if tx
            .send(TimelineCommand::ToggleReaction { event_id, key })
            .is_err()
        {
            tracing::debug!("timeline command channel closed");
        }
    }

    pub(super) fn jump_to_latest(&mut self, room_id: &RoomId, generation: i32) {
        if !self.is_current(room_id, generation) {
            return;
        }
        self.viewport.jump_to_latest();
        self.at_bottom = true;
        self.clear_new_messages(generation);
        self.emit_pagination_state();
        self.mark_read();
    }

    pub(super) fn scroll_position_changed(
        &mut self,
        room_id: &RoomId,
        generation: i32,
        at_bottom: bool,
    ) {
        if !self.is_current(room_id, generation) {
            return;
        }
        tracing::debug!(at_bottom, generation, "the timeline reported its position");

        let mode_changed = self.viewport.update_scroll_position(at_bottom);
        let reached_bottom = at_bottom && !self.at_bottom;

        self.at_bottom = at_bottom;

        if mode_changed && self.viewport.mode() == ScrollMode::FollowLive {
            self.clear_new_messages(generation);
        }

        if reached_bottom {
            self.mark_read();
        }
    }

    fn mark_read(&self) {
        if !self.live {
            return;
        }
        let Some(tx) = &self.timeline_cmd_tx else {
            return;
        };
        if tx.send(TimelineCommand::MarkRead).is_err() {
            tracing::debug!("timeline command channel closed");
        }
    }

    fn reset_state(&mut self) {
        self.viewport = ViewportController::new();
        self.timeline_cmd_tx = None;
        self.active_room_id = None;
        self.generation = 0;
        self.at_bottom = true;
        self.new_messages = 0;
        self.live = true;
    }

    fn emit_pagination_state(&self) {
        let generation = self.generation;
        let state = self.viewport.state();
        self.output.publish(Box::new(move |view| {
            view.pagination.retarget(generation);
            view.pagination.backwards_loading = state.backwards_loading;
            view.pagination.forwards_loading = state.forwards_loading;
        }));
    }

    async fn emit_reset(&self, room_id: RoomId, generation: i32, live: bool) {
        self.output
            .emit(Effect::TimelineStatus {
                room_id: room_id.clone(),
                generation,
                status: if live {
                    TimelineStatus::Loading
                } else {
                    TimelineStatus::LoadingFocus
                },
            })
            .await;
        self.output
            .emit(Effect::Timeline {
                room_id,
                generation,
                patch: Box::new(TimelinePatch::Clear),
            })
            .await;
    }

    fn emit_new_messages(&self, generation: i32, count: u32) {
        self.output.publish(Box::new(move |view| {
            view.pagination.retarget(generation);
            view.pagination.new_messages = count;
        }));
    }
}

struct Forwarder {
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    room_id: RoomId,
    generation: i32,
    live: bool,
    next_snapshot: Snapshot,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Snapshot {
    Opening,
    Replacement,
}

impl Forwarder {
    async fn run(&mut self, rx: &mut mpsc::Receiver<TimelineUpdate>) {
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

    async fn dispatch(&mut self, update: TimelineUpdate) -> bool {
        match update {
            TimelineUpdate::Patch(patch) => self.forward_patch(patch).await,
            TimelineUpdate::ResolvingUnread => {
                self.emit_status(TimelineStatus::LoadingUnread).await;
            }
            TimelineUpdate::JumpOutcome { event_id, target } => {
                self.forward_jump(event_id, target).await;
            }
            TimelineUpdate::AudioLocated { request, track } => {
                let located = TimelineEvent::AudioLocated {
                    room_id: self.room_id.clone(),
                    generation: self.generation,
                    request,
                    track,
                };
                if self.events.send(AppEvent::Timeline(located)).is_err() {
                    return false;
                }
            }
            TimelineUpdate::Pagination { direction, outcome } => {
                let settled = TimelineEvent::PaginationCompleted {
                    room_id: self.room_id.clone(),
                    generation: self.generation,
                    direction,
                    outcome,
                };
                if self.events.send(AppEvent::Timeline(settled)).is_err() {
                    return false;
                }
            }
        }
        true
    }

    async fn forward_patch(&mut self, patch: Box<TimelinePatch>) {
        if let Some(advance) = read_position_advance(patch.as_ref(), self.next_snapshot) {
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
                super::show_toast(
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

    fn send_advance(&self, advance: TimelineAdvance) {
        let advanced = TimelineEvent::Advanced {
            room_id: self.room_id.clone(),
            generation: self.generation,
            advance,
        };
        drop(self.events.send(AppEvent::Timeline(advanced)));
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

    fn widen_search_for(&self, event_id: String) {
        let searched_live_window = self.live;
        if searched_live_window {
            self.refocus(TimelineFocus::Event(event_id));
        } else {
            super::show_toast(
                self.output.as_ref(),
                Toast::Error(UserMessage::new(UserMessageKind::MessageNotFound)),
            );
        }
    }

    fn refocus(&self, focus: TimelineFocus) {
        let refocus = TimelineEvent::Refocus {
            room_id: self.room_id.clone(),
            generation: self.generation,
            focus,
        };
        drop(self.events.send(AppEvent::Timeline(refocus)));
    }
}

fn read_position_advance(patch: &TimelinePatch, snapshot: Snapshot) -> Option<TimelineAdvance> {
    if snapshot == Snapshot::Opening
        && let Some(anchor) = patch.unread_anchor()
    {
        return Some(TimelineAdvance::Anchored {
            count: anchor.count,
        });
    }

    let appended = count_appended(patch);
    let opens_room = patch.opens_room();
    if appended.total == 0 && !opens_room {
        return None;
    }
    Some(TimelineAdvance::Appended {
        total: appended.total,
        from_others: appended.from_others,
        opens_room,
    })
}

#[derive(Default, Clone, Copy)]
struct Appended {
    total: u32,
    from_others: bool,
}

impl Appended {
    fn merge(self, other: Self) -> Self {
        Self {
            total: self.total.saturating_add(other.total),
            from_others: self.from_others || other.from_others,
        }
    }
}

fn count_appended(patch: &TimelinePatch) -> Appended {
    match patch {
        TimelinePatch::Append(messages) => Appended {
            total: messages.len().try_into().unwrap_or(u32::MAX),
            from_others: messages.iter().any(|message| !message.is_own),
        },
        TimelinePatch::PushBack(message) => Appended {
            total: 1,
            from_others: !message.is_own,
        },
        TimelinePatch::Batch(patches) => patches
            .iter()
            .map(count_appended)
            .fold(Appended::default(), Appended::merge),
        _ => Appended::default(),
    }
}
