use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use tokio::sync::mpsc;

use super::task_group::TaskGroup;
use crate::commands::UiCommand;
use crate::domain::models::{
    PaginationDirection, PaginationOutcome, RoomId, ScrollMode, TimelineCommand, TimelinePatch,
    TimelineStatus, TimelineUpdate,
};
use crate::domain::viewport::ViewportController;
use crate::ports::matrix::MatrixPort;
use crate::ports::output::AppOutputPort;

const TIMELINE_CHANNEL_CAP: usize = 256;

pub(super) struct ActiveTimeline {
    matrix: Arc<dyn MatrixPort>,
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    output: Arc<dyn AppOutputPort>,
    tasks: TaskGroup,
    viewport: ViewportController,
    timeline_cmd_tx: Option<mpsc::UnboundedSender<TimelineCommand>>,
    active_room_id: Option<RoomId>,
    generation: i32,
    at_bottom: Arc<AtomicBool>,
    new_messages_counter: Arc<AtomicU32>,
}

impl ActiveTimeline {
    pub(super) fn new(
        matrix: Arc<dyn MatrixPort>,
        cmd_tx: mpsc::UnboundedSender<UiCommand>,
        output: Arc<dyn AppOutputPort>,
    ) -> Self {
        Self {
            matrix,
            cmd_tx,
            output,
            tasks: TaskGroup::new(),
            viewport: ViewportController::new(),
            timeline_cmd_tx: None,
            active_room_id: None,
            generation: 0,
            at_bottom: Arc::new(AtomicBool::new(true)),
            new_messages_counter: Arc::new(AtomicU32::new(0)),
        }
    }

    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
        self.reset_state();
    }

    pub(super) async fn select_room(&mut self, room_id: RoomId, generation: i32) {
        tracing::info!(%room_id, generation, "switching room");
        self.tasks.reset().await;

        self.viewport = ViewportController::new();
        self.active_room_id = Some(room_id.clone());
        self.generation = generation;
        self.at_bottom.store(true, Ordering::Relaxed);
        self.new_messages_counter.store(0, Ordering::Relaxed);
        self.emit_pagination_state(&room_id).await;

        self.output
            .timeline_status(room_id.clone(), TimelineStatus::Loading)
            .await;
        self.output
            .timeline(room_id.clone(), Box::new(TimelinePatch::Clear))
            .await;

        let (tl_tx, mut tl_rx) = mpsc::channel::<TimelineUpdate>(TIMELINE_CHANNEL_CAP);
        let (tl_cmd_tx, tl_cmd_rx) = mpsc::unbounded_channel::<TimelineCommand>();
        self.timeline_cmd_tx = Some(tl_cmd_tx);

        let matrix = Arc::clone(&self.matrix);
        let output = Arc::clone(&self.output);
        let cmd_tx = self.cmd_tx.clone();
        let token = self.tasks.token();
        let rid = room_id.clone();
        let at_bottom = Arc::clone(&self.at_bottom);
        let new_messages_counter = Arc::clone(&self.new_messages_counter);

        self.tasks.spawn(async move {
            let subscribe = matrix.subscribe_timeline(&room_id, tl_tx, tl_cmd_rx);
            let forward = async {
                while let Some(update) = tl_rx.recv().await {
                    tracing::debug!(
                        update = update.label(),
                        %rid,
                        "forwarding timeline update"
                    );

                    match update {
                        TimelineUpdate::Patch(patch) => {
                            if !at_bottom.load(Ordering::Relaxed) {
                                let added = count_appended(patch.as_ref());
                                if added > 0 {
                                    let prev =
                                        new_messages_counter.fetch_add(added, Ordering::Relaxed);
                                    let total = prev.saturating_add(added);
                                    output.new_messages_badge(rid.clone(), total).await;
                                }
                            }

                            output.timeline(rid.clone(), patch).await;
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
                        output.timeline_status(
                            rid.clone(),
                            TimelineStatus::Failed { retryable: true },
                        ).await;
                    } else {
                        tracing::debug!("timeline subscription ended");
                        output.timeline_status(rid.clone(), TimelineStatus::Disconnected).await;
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

    pub(super) async fn retry(&mut self) {
        let Some(room_id) = self.active_room_id.clone() else {
            return;
        };
        let generation = self.generation;
        self.select_room(room_id, generation).await;
    }

    pub(super) fn spawn_send(
        &self,
        group: &mut TaskGroup,
        room_id: RoomId,
        body: String,
        reply_to: Option<String>,
    ) {
        let matrix = Arc::clone(&self.matrix);
        let output = Arc::clone(&self.output);
        group.spawn(async move {
            let result = match reply_to {
                Some(event_id) => matrix.send_reply(&room_id, &body, &event_id).await,
                None => matrix.send_text(&room_id, &body).await,
            };
            if let Err(e) = result {
                tracing::warn!("failed to enqueue message: {e}");
                output
                    .notify_error(format!("Failed to send message: {e}"))
                    .await;
            }
        });
    }

    fn is_current(&self, room_id: &RoomId, generation: i32) -> bool {
        self.generation == generation && self.active_room_id.as_ref() == Some(room_id)
    }

    pub(super) async fn paginate_backwards(&mut self, room_id: &RoomId, generation: i32) {
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
        self.emit_pagination_state(room_id).await;
    }

    pub(super) async fn paginate_forwards(&mut self, room_id: &RoomId, generation: i32) {
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
        self.emit_pagination_state(room_id).await;
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
                    .notify_error("Failed to load more messages".to_owned())
                    .await;
                false
            }
        };
        self.emit_pagination_state(room_id).await;

        if matches!(direction, PaginationDirection::Forwards)
            && hit_end
            && self.at_bottom.load(Ordering::Relaxed)
        {
            self.new_messages_counter.store(0, Ordering::Relaxed);
            self.output.new_messages_badge(room_id.clone(), 0).await;
        }
    }

    pub(super) async fn jump_to_latest(&mut self, room_id: &RoomId, generation: i32) {
        if !self.is_current(room_id, generation) {
            return;
        }
        self.viewport.jump_to_latest();
        self.at_bottom.store(true, Ordering::Relaxed);
        self.new_messages_counter.store(0, Ordering::Relaxed);
        self.output.scroll_to_bottom(room_id.clone()).await;
        self.output.new_messages_badge(room_id.clone(), 0).await;
        self.emit_pagination_state(room_id).await;
    }

    pub(super) async fn scroll_position_changed(
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

        self.at_bottom.store(at_bottom, Ordering::Relaxed);

        if mode_changed && self.viewport.mode() == ScrollMode::FollowLive {
            self.new_messages_counter.store(0, Ordering::Relaxed);
            self.output.new_messages_badge(room_id.clone(), 0).await;
        }
    }

    fn reset_state(&mut self) {
        self.viewport = ViewportController::new();
        self.timeline_cmd_tx = None;
        self.active_room_id = None;
        self.generation = 0;
        self.at_bottom.store(true, Ordering::Relaxed);
        self.new_messages_counter.store(0, Ordering::Relaxed);
    }

    async fn emit_pagination_state(&self, room_id: &RoomId) {
        self.output
            .pagination_state(room_id.clone(), self.viewport.state())
            .await;
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
