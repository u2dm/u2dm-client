mod forwarder;

use std::sync::Arc;

use tokio::sync::mpsc;

use self::forwarder::{Forwarder, Snapshot};
use super::event::{AppEvent, TimelineEvent};
use super::input::EventSender;
use super::task_group::TaskGroup;
use crate::commands::effects::Effect;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::TimelineVisibility;
use crate::commands::view::Toast;
use crate::domain::poll::{PollAction, PollDraft};
use crate::domain::room::RoomId;
use crate::domain::settings::{ReceiptVisibility, RowFilter};
use crate::domain::timeline::{
    AudioLookup, FailedSend, PaginationDirection, PaginationOutcome, TimelineAdvance,
    TimelineCommand, TimelineFocus, TimelinePatch, TimelineStatus, TimelineUpdate,
};
use crate::domain::viewport::ViewportController;
use crate::ports::matrix::TimelinePort;
use crate::ports::output::AppOutputPort;

const TIMELINE_CHANNEL_CAP: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadBoundary {
    Resolved,
    Unresolved,
}

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
    visibility: TimelineVisibility,
    receipt_withheld: bool,
    read_boundary: ReadBoundary,
    receipts: ReceiptVisibility,
    rows: RowFilter,
}

impl ActiveTimeline {
    pub(super) fn new(events: EventSender, output: Arc<dyn AppOutputPort>) -> Self {
        Self {
            events,
            output,
            tasks: TaskGroup::new("timeline"),
            viewport: ViewportController::default(),
            timeline_cmd_tx: None,
            active_room_id: None,
            generation: 0,
            at_bottom: true,
            new_messages: 0,
            live: true,
            visibility: TimelineVisibility::default(),
            receipt_withheld: false,
            read_boundary: ReadBoundary::Resolved,
            receipts: ReceiptVisibility::default(),
            rows: RowFilter::default(),
        }
    }

    pub(super) const fn set_receipts(&mut self, receipts: ReceiptVisibility) {
        self.receipts = receipts;
    }

    pub(super) fn set_row_filter(&mut self, rows: RowFilter) {
        if self.rows == rows {
            return;
        }
        tracing::info!(?rows, "the timeline shows a different set of rows");
        self.rows = rows;
        self.forward(TimelineCommand::Reclassify(rows));
    }

    pub(super) fn recheck_media(&self) {
        self.forward(TimelineCommand::RecheckMedia);
    }

    pub(super) fn load_media(&self, event_id: String) {
        self.forward(TimelineCommand::LoadMedia { event_id });
    }

    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
        self.reset_state();
        self.visibility = TimelineVisibility::default();
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
        self.viewport = ViewportController::new(&focus);
        self.active_room_id = Some(room_id.clone());
        self.generation = generation;
        self.at_bottom = true;
        self.new_messages = 0;
        self.live = live;
        self.receipt_withheld = false;
        self.read_boundary = ReadBoundary::Resolved;
        self.emit_pagination_state();

        self.emit_reset(room_id.clone(), generation, live).await;

        let (tl_tx, mut tl_rx) = mpsc::channel::<TimelineUpdate>(TIMELINE_CHANNEL_CAP);
        let (tl_cmd_tx, tl_cmd_rx) = mpsc::unbounded_channel::<TimelineCommand>();
        self.timeline_cmd_tx = Some(tl_cmd_tx);

        let token = self.tasks.token();
        let rows = self.rows;
        let mut forwarder = Forwarder {
            output: Arc::clone(&self.output),
            events: self.events.clone(),
            room_id: room_id.clone(),
            generation,
            focus: focus.clone(),
            next_snapshot: Snapshot::Opening,
        };

        self.tasks.spawn(async move {
            let mut subscribe =
                timeline.subscribe_timeline(&room_id, focus, rows, tl_tx, tl_cmd_rx, token.clone());

            tokio::select! {
                biased;
                () = token.cancelled() => {
                    tracing::debug!("timeline subscription cancelled");
                    forwarder.report_failures_until(subscribe, &mut tl_rx).await;
                }
                result = &mut subscribe => forwarder.subscription_ended(result).await,
                () = forwarder.run(&mut tl_rx) => {
                    tracing::debug!("timeline forwarder stopped");
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

    pub(super) fn on_screen(&self) -> Option<&RoomId> {
        self.active_room_id
            .as_ref()
            .filter(|_| self.visibility == TimelineVisibility::Visible)
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
        self.viewport.start_backwards();
        if tx.send(TimelineCommand::PaginateBackwards).is_err() {
            tracing::debug!("timeline command channel closed");
            self.viewport
                .fail_pagination(PaginationDirection::Backwards);
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
            self.refocus(room_id, generation, TimelineFocus::ReadPosition);
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
            TimelineAdvance::UnreadUnresolved => {
                self.read_boundary = ReadBoundary::Unresolved;
            }
            TimelineAdvance::Anchored { count } => {
                self.at_bottom = false;
                self.add_new_messages(generation, count);
            }
            TimelineAdvance::Arrived {
                arrivals,
                opens_room,
            } => {
                if self.at_bottom {
                    if opens_room || arrivals.from_others {
                        self.mark_read_if_resolved();
                    }
                } else if arrivals.new_messages > 0 {
                    self.add_new_messages(generation, arrivals.new_messages);
                }
            }
        }
    }

    fn add_new_messages(&mut self, generation: i32, count: u32) {
        self.set_new_messages(generation, self.new_messages.saturating_add(count));
    }

    fn lower_new_messages(&mut self, generation: i32, remaining: u32) {
        self.set_new_messages(generation, self.new_messages.min(remaining));
    }

    fn clear_new_messages(&mut self, generation: i32) {
        self.set_new_messages(generation, 0);
    }

    fn set_new_messages(&mut self, generation: i32, count: u32) {
        if self.new_messages == count {
            return;
        }
        self.new_messages = count;
        self.emit_new_messages(generation, count);
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

    pub(super) fn locate_source(&self, request: u64, event_id: String) -> bool {
        self.timeline_cmd_tx.as_ref().is_some_and(|tx| {
            tx.send(TimelineCommand::LocateSource { request, event_id })
                .is_ok()
        })
    }

    pub(super) fn locate_readers(&self, request: u64, event_id: String) -> bool {
        self.timeline_cmd_tx.as_ref().is_some_and(|tx| {
            tx.send(TimelineCommand::LocateReaders { request, event_id })
                .is_ok()
        })
    }

    pub(super) fn is_active_room(&self, room_id: &RoomId) -> bool {
        self.active_room_id.as_ref() == Some(room_id)
    }

    pub(super) fn toggle_reaction(
        &mut self,
        event_id: String,
        key: String,
        shortcode: Option<String>,
    ) {
        self.forward(TimelineCommand::ToggleReaction {
            event_id,
            key,
            shortcode,
        });
    }

    pub(super) fn vote_poll(&mut self, event_id: String, answer_id: String) {
        self.forward(TimelineCommand::VotePoll {
            event_id,
            answer_id,
        });
    }

    pub(super) fn end_poll(&mut self, event_id: String) {
        self.forward(TimelineCommand::EndPoll { event_id });
    }

    pub(super) fn edit_poll(&mut self, event_id: String, draft: PollDraft) {
        self.forward(TimelineCommand::EditPoll { event_id, draft });
    }

    fn forward(&self, command: TimelineCommand) {
        let Some(tx) = &self.timeline_cmd_tx else {
            return;
        };
        if tx.send(command).is_err() {
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
        self.mark_read_at_latest();
    }

    pub(super) fn scroll_position_changed(
        &mut self,
        room_id: &RoomId,
        generation: i32,
        at_bottom: bool,
        unread_below: u32,
    ) {
        if !self.is_current(room_id, generation) {
            return;
        }
        tracing::debug!(
            at_bottom,
            unread_below,
            generation,
            "the timeline reported its position"
        );

        self.viewport.update_scroll_position(at_bottom);
        let reached_bottom = at_bottom && !self.at_bottom;

        self.at_bottom = at_bottom;

        if at_bottom {
            self.clear_new_messages(generation);
        } else {
            self.lower_new_messages(generation, unread_below);
        }

        if reached_bottom {
            self.mark_read_at_latest();
        }
    }

    pub(super) fn visibility_changed(&mut self, visibility: TimelineVisibility) {
        tracing::debug!(?visibility, "the timeline's visibility changed");
        self.visibility = visibility;
        if self.receipt_withheld && self.at_bottom {
            self.mark_read();
        }
    }

    fn mark_read_if_resolved(&mut self) {
        if self.read_boundary == ReadBoundary::Unresolved {
            tracing::debug!("withholding the read receipt until the unread boundary resolves");
            return;
        }
        self.mark_read();
    }

    fn mark_read_at_latest(&mut self) {
        self.read_boundary = ReadBoundary::Resolved;
        self.mark_read();
    }

    fn mark_read(&mut self) {
        if !self.live {
            return;
        }
        if self.visibility == TimelineVisibility::Hidden {
            tracing::debug!("withholding the read receipt until the timeline is visible");
            self.receipt_withheld = true;
            return;
        }
        let Some(tx) = &self.timeline_cmd_tx else {
            return;
        };
        self.receipt_withheld = false;
        tracing::debug!("marking the room read");
        if tx.send(TimelineCommand::MarkRead(self.receipts)).is_err() {
            tracing::debug!("timeline command channel closed");
        }
    }

    fn reset_state(&mut self) {
        self.viewport = ViewportController::default();
        self.timeline_cmd_tx = None;
        self.active_room_id = None;
        self.generation = 0;
        self.at_bottom = true;
        self.new_messages = 0;
        self.live = true;
        self.receipt_withheld = false;
        self.read_boundary = ReadBoundary::Resolved;
    }

    fn emit_pagination_state(&self) {
        let generation = self.generation;
        let state = self.viewport.state();
        self.output.publish(Box::new(move |view| {
            view.pagination.retarget(generation);
            view.pagination.older_history = state.older_history;
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

fn report_poll_failure(output: &dyn AppOutputPort, action: PollAction) {
    let kind = match action {
        PollAction::Vote => UserMessageKind::PollVoteFailed,
        PollAction::End => UserMessageKind::PollEndFailed,
        PollAction::Edit => UserMessageKind::PollEditFailed,
    };
    super::show_toast(output, Toast::Error(UserMessage::new(kind)));
}

pub(super) fn report_delete_failure(output: &dyn AppOutputPort) {
    super::show_toast(
        output,
        Toast::Error(UserMessage::new(UserMessageKind::MessageDeleteFailed)),
    );
}
