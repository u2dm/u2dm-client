use std::ops::Range;

use tokio::sync::mpsc;
use tokio::time::sleep;

use super::deletions::stamp_deleted_reply;
use super::sends::send_patch;
use crate::adapters::demo::matrix::{ActiveRoom, DemoAuthed, unavailable};
use crate::adapters::demo::{data, polls, reactions, receipts, room_info, timeline};
use crate::domain::message::{SendState, TimelineMessage};
use crate::domain::room::RoomId;
use crate::domain::timeline::{
    JumpTarget, PaginationDirection, PaginationOutcome, TimelineFocus, TimelinePatch,
    TimelineUpdate,
};
use crate::error::{AppError, Result};

pub(super) struct OpenedWindow {
    pub(super) all: Vec<TimelineMessage>,
    pub(super) newer: usize,
    pub(super) messages: Vec<TimelineMessage>,
    pub(super) reset_messages: Vec<TimelineMessage>,
}

pub(super) struct Paging {
    pub(super) returns_history: bool,
    pub(super) history: u64,
    pub(super) messages: Vec<TimelineMessage>,
    pub(super) all: Vec<TimelineMessage>,
    pub(super) newer: usize,
}

impl DemoAuthed {
    pub(in crate::adapters::demo::matrix) fn room_history(
        &self,
        room_id: &RoomId,
    ) -> Vec<TimelineMessage> {
        let mut history = data::messages(room_id);
        if let Ok(arrived) = self.arrived.lock()
            && let Some(messages) = arrived.get(room_id)
        {
            history.extend(messages.iter().cloned());
        }
        if let Ok(own_sends) = self.own_sends.lock()
            && let Some(sent) = own_sends.get(room_id)
        {
            history.extend(sent.iter().cloned());
        }
        if let Ok(deleted) = self.deleted.lock()
            && !deleted.is_empty()
        {
            history.retain(|message| {
                message
                    .event_id
                    .as_ref()
                    .is_none_or(|event_id| !deleted.contains(event_id))
            });
            for message in &mut history {
                stamp_deleted_reply(message, &deleted);
            }
        }
        if let Ok(ignored) = self.ignored.lock()
            && !ignored.is_empty()
        {
            history.retain(|message| !ignored.contains(&message.sender));
        }
        history
    }

    pub(in crate::adapters::demo::matrix) fn replay_live_window(&self) {
        let Some((timeline_tx, messages)) = self.refiltered_window() else {
            return;
        };
        tokio::spawn(async move {
            sleep(room_info::ECHO_LAG).await;
            send_patch(&timeline_tx, TimelinePatch::Reset(messages)).await;
        });
    }

    fn refiltered_window(&self) -> Option<(mpsc::Sender<TimelineUpdate>, Vec<TimelineMessage>)> {
        let mut guard = self.active.lock().ok()?;
        let active = guard.as_mut().filter(|active| active.live)?;
        active.messages = self.room_history(&active.room_id);
        active.prepended = 0;
        Some((active.timeline_tx.clone(), active.messages.clone()))
    }

    pub(super) async fn open_window(
        &self,
        room_id: &RoomId,
        focus: &TimelineFocus,
        scenario: timeline::Scenario,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) -> Result<Option<OpenedWindow>> {
        let Ok(reset_slot) = timeline_tx.reserve().await else {
            return Ok(None);
        };
        let Ok(mut active) = self.active.lock() else {
            return Err(unavailable("the demo timeline"));
        };
        let all = self.room_history(room_id);
        let window = opening_window(&all, focus, scenario)?;
        let newer = window.end;
        let messages = window_messages(&all, window, focus);
        let reset_messages = reset_messages(&messages);
        let reset = opening_patch(scenario, reset_messages.clone());
        reset_slot.send(TimelineUpdate::patch(reset));
        *active = Some(ActiveRoom {
            room_id: room_id.clone(),
            live: focus.is_live(),
            timeline_tx: timeline_tx.clone(),
            messages: messages.clone(),
            prepended: 0,
            receipts: receipts::seed_receipts(&all),
        });
        Ok(Some(OpenedWindow {
            all,
            newer,
            messages,
            reset_messages,
        }))
    }

    pub(super) fn loaded_row_of(&self, event_id: &str) -> JumpTarget {
        let row = self.active.lock().ok().and_then(|guard| {
            let active = guard.as_ref()?;
            let index_in_window = active
                .messages
                .iter()
                .position(|message| message.event_id.as_deref() == Some(event_id))?;
            Some(active.prepended.saturating_add(index_in_window))
        });
        row.map_or(JumpTarget::NotLoaded, JumpTarget::Row)
    }

    async fn append_newer(
        &self,
        page: Vec<TimelineMessage>,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) {
        if page.is_empty() {
            return;
        }
        if let Ok(mut guard) = self.active.lock()
            && let Some(active) = guard.as_mut()
        {
            active.messages.extend(page.iter().cloned());
        }
        send_patch(timeline_tx, TimelinePatch::Append(page)).await;
    }

    pub(super) async fn announce_opening(
        &self,
        focus: &TimelineFocus,
        scenario: timeline::Scenario,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) {
        let opening_page = TimelineUpdate::Pagination {
            direction: PaginationDirection::Backwards,
            outcome: PaginationOutcome::Completed {
                hit_end: !scenario.pagination_returns_history,
            },
        };
        drop(timeline_tx.send(opening_page).await);

        if let Some(target) = focus.target() {
            let target_row = self.loaded_row_of(target);
            drop(
                timeline_tx
                    .send(TimelineUpdate::JumpOutcome {
                        event_id: target.to_owned(),
                        target: target_row,
                    })
                    .await,
            );
        }
    }

    pub(super) async fn page_in(
        &self,
        direction: PaginationDirection,
        paging: &mut Paging,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) -> bool {
        match direction {
            PaginationDirection::Backwards if paging.returns_history => {
                paging.history += 1;
                let page = older_history(paging.history, &paging.messages);
                if let Ok(mut guard) = self.active.lock()
                    && let Some(active) = guard.as_mut()
                {
                    active.prepended = active.prepended.saturating_add(page.len());
                }
                let page = page.into_iter().map(TimelinePatch::PushFront).collect();
                send_patch(timeline_tx, TimelinePatch::Batch(page)).await;
                paging.history > 2
            }
            PaginationDirection::Backwards => true,
            PaginationDirection::Forwards => {
                let page = newer_page(&paging.all, &mut paging.newer);
                let hit_end = page.is_empty();
                self.append_newer(page, timeline_tx).await;
                hit_end
            }
        }
    }
}

pub(super) fn reset_messages(messages: &[TimelineMessage]) -> Vec<TimelineMessage> {
    let messages = if reactions::scenario().reactions_arrive_late {
        reactions::strip_reactions(messages)
    } else {
        messages.to_vec()
    };
    let messages = if polls::scenario().tallies_arrive_late {
        polls::strip_tallies(&messages)
    } else {
        messages
    };
    if receipts::scenario().marks_arrive_late {
        receipts::strip_read_marks(&messages)
    } else {
        messages
    }
}

fn opening_window(
    all: &[TimelineMessage],
    focus: &TimelineFocus,
    scenario: timeline::Scenario,
) -> Result<Range<usize>> {
    match focus.target() {
        Some(target) => focused_window(all, target),
        None if scenario.window_is_short => Ok(newest(all, timeline::SHORT_WINDOW)),
        None => Ok(0..all.len()),
    }
}

fn window_messages(
    all: &[TimelineMessage],
    window: Range<usize>,
    focus: &TimelineFocus,
) -> Vec<TimelineMessage> {
    let mut messages = all.get(window).unwrap_or(all).to_vec();
    if matches!(focus, TimelineFocus::Latest) {
        clear_unread_divider(&mut messages);
    }
    messages
}

fn clear_unread_divider(messages: &mut [TimelineMessage]) {
    for message in messages {
        message.is_first_unread = false;
    }
}

fn newest(messages: &[TimelineMessage], count: usize) -> Range<usize> {
    messages.len().saturating_sub(count)..messages.len()
}

fn focused_window(messages: &[TimelineMessage], target: &str) -> Result<Range<usize>> {
    let index = messages
        .iter()
        .position(|message| message.event_id.as_deref() == Some(target))
        .ok_or(AppError::FocusedEventUnavailable)?;
    let start = index.saturating_sub(timeline::FOCUS_CONTEXT);
    let end = messages
        .len()
        .min(index.saturating_add(timeline::FOCUS_CONTEXT));
    Ok(start..end)
}

fn newer_page(all: &[TimelineMessage], newer: &mut usize) -> Vec<TimelineMessage> {
    let end = all.len().min(newer.saturating_add(timeline::NEWER_PAGE));
    let page = all.get(*newer..end).unwrap_or_default().to_vec();
    *newer = end;
    page
}

pub(in crate::adapters::demo::matrix) fn opening_patch(
    scenario: timeline::Scenario,
    messages: Vec<TimelineMessage>,
) -> TimelinePatch {
    let reset = TimelinePatch::Reset(messages);
    if scenario.reset_is_batched {
        return TimelinePatch::Batch(vec![reset]);
    }
    reset
}

fn older_history(round: u64, messages: &[TimelineMessage]) -> Vec<TimelineMessage> {
    messages
        .iter()
        .take(timeline::HISTORY_PAGE)
        .enumerate()
        .map(|(index, message)| {
            let id = format!("demo-history-{round}-{index}");
            TimelineMessage {
                unique_id: id.clone(),
                event_id: Some(id),
                local_id: None,
                is_first_unread: false,
                send_state: SendState::default(),
                ..message.clone()
            }
        })
        .collect()
}
