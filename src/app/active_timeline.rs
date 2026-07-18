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

pub(super) struct ActiveTimeline {
    matrix: Arc<dyn MatrixPort>,
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    output: Arc<dyn AppOutputPort>,
    tasks: TaskGroup,
    viewport: ViewportController,
    timeline_cmd_tx: Option<mpsc::UnboundedSender<TimelineCommand>>,
    active_room_id: Option<RoomId>,
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
            at_bottom: Arc::new(AtomicBool::new(true)),
            new_messages_counter: Arc::new(AtomicU32::new(0)),
        }
    }

    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
        self.reset_state();
    }

    pub(super) async fn select_room(&mut self, room_id: RoomId) {
        tracing::info!(%room_id, "switching room");
        self.tasks.reset().await;

        self.viewport = ViewportController::new();
        self.active_room_id = Some(room_id.clone());
        self.at_bottom.store(true, Ordering::Relaxed);
        self.new_messages_counter.store(0, Ordering::Relaxed);
        self.emit_pagination_state(&room_id);

        self.output
            .timeline_status(room_id.clone(), TimelineStatus::Loading);
        self.output
            .timeline(room_id.clone(), Box::new(TimelinePatch::Clear));

        let (tl_tx, mut tl_rx) = mpsc::unbounded_channel::<TimelineUpdate>();
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
                                    output.new_messages_badge(rid.clone(), total);
                                }
                            }

                            output.timeline(rid.clone(), patch);
                        }
                        TimelineUpdate::Pagination { direction, outcome } => {
                            if let Err(e) = cmd_tx.send(UiCommand::TimelinePaginationCompleted {
                                room_id: rid.clone(),
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
                        );
                    } else {
                        tracing::debug!("timeline subscription ended");
                        output.timeline_status(rid.clone(), TimelineStatus::Disconnected);
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

    pub(super) async fn send_message(
        &self,
        room_id: RoomId,
        body: String,
        reply_to: Option<String>,
    ) {
        let result = match reply_to {
            Some(event_id) => self.matrix.send_reply(&room_id, &body, &event_id).await,
            None => self.matrix.send_text(&room_id, &body).await,
        };
        if let Err(e) = result {
            tracing::warn!("failed to enqueue message: {e}");
            self.output
                .notify_error(format!("Failed to send message: {e}"));
        }
    }

    pub(super) fn paginate_backwards(&mut self, room_id: &RoomId) {
        if self.active_room_id.as_ref() != Some(room_id) {
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
        self.emit_pagination_state(room_id);
    }

    pub(super) fn paginate_forwards(&mut self, room_id: &RoomId) {
        if self.active_room_id.as_ref() != Some(room_id) {
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
        self.emit_pagination_state(room_id);
    }

    pub(super) fn complete_pagination(
        &mut self,
        room_id: &RoomId,
        direction: PaginationDirection,
        outcome: PaginationOutcome,
    ) {
        if self.active_room_id.as_ref() != Some(room_id) {
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
                    .notify_error("Failed to load more messages".to_owned());
                false
            }
        };
        self.emit_pagination_state(room_id);

        if matches!(direction, PaginationDirection::Forwards)
            && hit_end
            && self.at_bottom.load(Ordering::Relaxed)
        {
            self.new_messages_counter.store(0, Ordering::Relaxed);
            self.output.new_messages_badge(room_id.clone(), 0);
        }
    }

    pub(super) fn jump_to_latest(&mut self, room_id: &RoomId) {
        if self.active_room_id.as_ref() != Some(room_id) {
            return;
        }
        self.viewport.jump_to_latest();
        self.at_bottom.store(true, Ordering::Relaxed);
        self.new_messages_counter.store(0, Ordering::Relaxed);
        self.output.scroll_to_bottom(room_id.clone());
        self.output.new_messages_badge(room_id.clone(), 0);
        self.emit_pagination_state(room_id);
    }

    pub(super) fn scroll_position_changed(&mut self, at_top: bool, at_bottom: bool) {
        let mode_changed = self.viewport.update_scroll_position(at_top, at_bottom);

        self.at_bottom.store(at_bottom, Ordering::Relaxed);

        let Some(room_id) = self.active_room_id.clone() else {
            return;
        };

        if mode_changed && self.viewport.mode() == ScrollMode::FollowLive {
            self.new_messages_counter.store(0, Ordering::Relaxed);
            self.output.new_messages_badge(room_id, 0);
        }
    }

    fn reset_state(&mut self) {
        self.viewport = ViewportController::new();
        self.timeline_cmd_tx = None;
        self.active_room_id = None;
        self.at_bottom.store(true, Ordering::Relaxed);
        self.new_messages_counter.store(0, Ordering::Relaxed);
    }

    fn emit_pagination_state(&self, room_id: &RoomId) {
        self.output
            .pagination_state(room_id.clone(), self.viewport.state());
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
