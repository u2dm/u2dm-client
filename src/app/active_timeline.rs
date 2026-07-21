use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use tokio::sync::mpsc;

use super::task_group::TaskGroup;
use crate::commands::{Effect, UiCommand};
use crate::domain::models::{
    PaginationDirection, PaginationOutcome, RoomId, ScrollMode, TimelineCommand, TimelinePatch,
    TimelineStatus, TimelineUpdate,
};
use crate::domain::viewport::ViewportController;
use crate::ports::matrix::TimelinePort;
use crate::ports::output::AppOutputPort;

const TIMELINE_CHANNEL_CAP: usize = 256;

#[derive(Clone)]
struct GenerationCounters {
    at_bottom: Arc<AtomicBool>,
    new_messages: Arc<AtomicU32>,
}

impl GenerationCounters {
    fn new() -> Self {
        Self {
            at_bottom: Arc::new(AtomicBool::new(true)),
            new_messages: Arc::new(AtomicU32::new(0)),
        }
    }

    fn is_at_bottom(&self) -> bool {
        self.at_bottom.load(Ordering::Relaxed)
    }

    fn set_at_bottom(&self, at_bottom: bool) {
        self.at_bottom.store(at_bottom, Ordering::Relaxed);
    }

    fn add_new_messages(&self, count: u32) -> u32 {
        self.new_messages
            .fetch_add(count, Ordering::Relaxed)
            .saturating_add(count)
    }

    fn clear_new_messages(&self) {
        self.new_messages.store(0, Ordering::Relaxed);
    }
}

pub(super) struct ActiveTimeline {
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    output: Arc<dyn AppOutputPort>,
    tasks: TaskGroup,
    viewport: ViewportController,
    timeline_cmd_tx: Option<mpsc::UnboundedSender<TimelineCommand>>,
    active_room_id: Option<RoomId>,
    generation: i32,
    counters: GenerationCounters,
}

impl ActiveTimeline {
    pub(super) fn new(
        cmd_tx: mpsc::UnboundedSender<UiCommand>,
        output: Arc<dyn AppOutputPort>,
    ) -> Self {
        Self {
            cmd_tx,
            output,
            tasks: TaskGroup::new("timeline"),
            viewport: ViewportController::new(),
            timeline_cmd_tx: None,
            active_room_id: None,
            generation: 0,
            counters: GenerationCounters::new(),
        }
    }

    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
        self.reset_state();
    }

    pub(super) async fn select_room(
        &mut self,
        timeline: Arc<dyn TimelinePort>,
        room_id: RoomId,
        generation: i32,
    ) {
        tracing::info!(%room_id, generation, "switching room");
        self.tasks.cancel_and_detach();

        self.viewport = ViewportController::new();
        self.active_room_id = Some(room_id.clone());
        self.generation = generation;
        self.counters = GenerationCounters::new();
        self.emit_pagination_state();

        self.emit_reset(room_id.clone(), generation).await;

        let (tl_tx, mut tl_rx) = mpsc::channel::<TimelineUpdate>(TIMELINE_CHANNEL_CAP);
        let (tl_cmd_tx, tl_cmd_rx) = mpsc::unbounded_channel::<TimelineCommand>();
        self.timeline_cmd_tx = Some(tl_cmd_tx);

        let output = Arc::clone(&self.output);
        let cmd_tx = self.cmd_tx.clone();
        let token = self.tasks.token();
        let rid = room_id.clone();
        let counters = self.counters.clone();

        self.tasks.spawn(async move {
            let subscribe = timeline.subscribe_timeline(&room_id, tl_tx, tl_cmd_rx);
            let forward = async {
                while let Some(update) = tl_rx.recv().await {
                    tracing::debug!(
                        update = update.label(),
                        %rid,
                        "forwarding timeline update"
                    );

                    match update {
                        TimelineUpdate::Patch(patch) => {
                            if !counters.is_at_bottom() {
                                let added = count_appended(patch.as_ref());
                                if added > 0 {
                                    let total = counters.add_new_messages(added);
                                    output.publish(Box::new(move |view| {
                                        view.pagination.retarget(generation);
                                        view.pagination.new_messages = total;
                                    }));
                                }
                            }

                            output
                                .emit(Effect::Timeline {
                                    room_id: rid.clone(),
                                    generation,
                                    patch,
                                })
                                .await;
                        }
                        TimelineUpdate::Pagination { direction, outcome } => {
                            if let Err(e) = cmd_tx.send(UiCommand::TimelinePaginationCompleted {
                                room_id: rid.clone(),
                                generation,
                                direction,
                                outcome,
                            }) {
                                tracing::debug!(
                                    "failed to send TimelinePaginationCompleted command: {e}"
                                );
                                break;
                            }
                        }
                    }
                }
            };

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

    pub(super) fn spawn_send(
        &self,
        group: &mut TaskGroup,
        timeline: Arc<dyn TimelinePort>,
        room_id: RoomId,
        body: String,
        reply_to: Option<String>,
    ) {
        let output = Arc::clone(&self.output);
        group.spawn(async move {
            let result = match reply_to {
                Some(event_id) => timeline.send_reply(&room_id, &body, &event_id).await,
                None => timeline.send_text(&room_id, &body).await,
            };
            if let Err(e) = result {
                tracing::warn!("failed to enqueue message: {e}");
                output
                    .emit(Effect::Toast(format!("Failed to send message: {e}")))
                    .await;
            }
        });
    }

    fn is_current(&self, room_id: &RoomId, generation: i32) -> bool {
        self.generation == generation && self.active_room_id.as_ref() == Some(room_id)
    }

    pub(super) fn paginate_backwards(&mut self, room_id: &RoomId, generation: i32) {
        if !self.is_current(room_id, generation) {
            return;
        }
        if !self.viewport.should_paginate_backwards(true) {
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
        if !self.viewport.should_paginate_forwards(true) {
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

    pub(super) async fn complete_pagination(
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
                self.output
                    .emit(Effect::Toast("Failed to load more messages".to_owned()))
                    .await;
                false
            }
        };
        self.emit_pagination_state();

        if matches!(direction, PaginationDirection::Forwards)
            && hit_end
            && self.counters.is_at_bottom()
        {
            self.counters.clear_new_messages();
            self.emit_new_messages(generation, 0);
        }
    }

    pub(super) fn jump_to_latest(&mut self, room_id: &RoomId, generation: i32) {
        if !self.is_current(room_id, generation) {
            return;
        }
        self.viewport.jump_to_latest();
        self.counters.set_at_bottom(true);
        self.counters.clear_new_messages();
        self.emit_new_messages(generation, 0);
        self.emit_pagination_state();
    }

    pub(super) fn scroll_position_changed(
        &mut self,
        room_id: &RoomId,
        generation: i32,
        at_top: bool,
        at_bottom: bool,
    ) {
        if !self.is_current(room_id, generation) {
            return;
        }

        let mode_changed = self.viewport.update_scroll_position(at_top, at_bottom);

        self.counters.set_at_bottom(at_bottom);

        if mode_changed && self.viewport.mode() == ScrollMode::FollowLive {
            self.counters.clear_new_messages();
            self.emit_new_messages(generation, 0);
        }
    }

    fn reset_state(&mut self) {
        self.viewport = ViewportController::new();
        self.timeline_cmd_tx = None;
        self.active_room_id = None;
        self.generation = 0;
        self.counters = GenerationCounters::new();
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

    async fn emit_reset(&self, room_id: RoomId, generation: i32) {
        self.output
            .emit(Effect::TimelineStatus {
                room_id: room_id.clone(),
                generation,
                status: TimelineStatus::Loading,
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

fn count_appended(patch: &TimelinePatch) -> u32 {
    match patch {
        TimelinePatch::Append(messages) => messages.len().try_into().unwrap_or(u32::MAX),
        TimelinePatch::PushBack(_) => 1,
        TimelinePatch::Batch(patches) => patches.iter().map(count_appended).sum(),
        _ => 0,
    }
}
