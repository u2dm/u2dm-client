use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::sync::mpsc;

use super::sends::send_patch;
use crate::adapters::demo::matrix::timeline_tasks::spawn_poll_refusal;
use crate::adapters::demo::matrix::{DemoAuthed, with_room_facts};
use crate::adapters::demo::{data, polls, powers};
use crate::adapters::draft_size;
use crate::domain::event_size::EventSize;
use crate::domain::message::TimelineMessage;
use crate::domain::poll::{PollAction, PollDraft};
use crate::domain::room::RoomId;
use crate::domain::timeline::{TimelinePatch, TimelineUpdate};

impl DemoAuthed {
    pub(super) async fn append_own_poll(&self, room_id: &RoomId, draft: &PollDraft) {
        let Some((timeline_tx, _, message)) = self.append_own(room_id, |_| {
            data::own_poll(
                self.sent.fetch_add(1, Ordering::Relaxed),
                draft,
                self.send_state_in(room_id),
            )
        }) else {
            return;
        };
        send_patch(&timeline_tx, TimelinePatch::PushBack(message)).await;
    }

    fn patch_poll(
        &self,
        event_id: &str,
        change: impl FnOnce(&mut TimelineMessage) -> bool,
    ) -> Option<(mpsc::Sender<TimelineUpdate>, usize, TimelineMessage)> {
        let mut guard = self.active.lock().ok()?;
        let active = guard.as_mut()?;
        let offset = active
            .messages
            .iter()
            .position(|message| message.event_id.as_deref() == Some(event_id))?;
        let row = active.prepended.saturating_add(offset);
        let message = active.messages.get_mut(offset)?;
        change(message).then(|| (active.timeline_tx.clone(), row, message.clone()))
    }

    pub(super) async fn vote_poll(
        &self,
        event_id: &str,
        answer_id: &str,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) {
        if !powers::polls().vote {
            refuse_poll_action(timeline_tx, PollAction::Vote).await;
            return;
        }
        let mut previous = None;
        let Some((timeline_tx, index, message)) = self.patch_poll(event_id, |message| {
            previous = polls::cast(message, answer_id);
            previous.is_some()
        }) else {
            tracing::debug!(event_id, "demo: a vote that changes nothing");
            return;
        };
        send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        if polls::scenario().votes_fail
            && let Some(previous) = previous
        {
            spawn_poll_refusal(
                Arc::clone(&self.active),
                timeline_tx,
                event_id.to_owned(),
                Some(polls::Undo::Selection(previous)),
                PollAction::Vote,
            );
        }
    }

    pub(super) async fn end_poll(
        &self,
        event_id: &str,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) {
        if !powers::polls().end {
            refuse_poll_action(timeline_tx, PollAction::End).await;
            return;
        }
        let mut editable = false;
        let Some((timeline_tx, index, message)) = self.patch_poll(event_id, |message| {
            editable = message.body.poll().is_some_and(|poll| poll.editable);
            polls::end_own(message)
        }) else {
            tracing::debug!(event_id, "demo: only an own open poll can be ended");
            return;
        };
        send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        if polls::scenario().ends_fail {
            spawn_poll_refusal(
                Arc::clone(&self.active),
                timeline_tx,
                event_id.to_owned(),
                Some(polls::Undo::End { editable }),
                PollAction::End,
            );
        }
    }

    pub(super) async fn edit_poll(
        &self,
        event_id: &str,
        draft: &PollDraft,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) {
        if !powers::polls().start {
            refuse_poll_action(timeline_tx, PollAction::Edit).await;
            return;
        }
        let measured = self.active_room_id().map(|room_id| {
            with_room_facts(&room_id, |facts| {
                draft_size::poll_edit(draft, event_id, facts)
            })
        });
        if let Some(Err(oversize)) = measured.map(EventSize::checked) {
            tracing::debug!(event_id, %oversize, "demo: the poll edit is over the size limit");
            refuse_poll_action(timeline_tx, PollAction::Edit).await;
            return;
        }
        let sequence = self.sent.fetch_add(1, Ordering::Relaxed);
        let mut revised = polls::Revised::Refused;
        let Some((timeline_tx, index, message)) = self.patch_poll(event_id, |message| {
            revised = polls::revise(message, draft, sequence);
            revised == polls::Revised::Applied
        }) else {
            if revised == polls::Revised::Refused {
                refuse_poll_action(timeline_tx, PollAction::Edit).await;
            }
            return;
        };
        send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        if polls::scenario().edits_fail {
            spawn_poll_refusal(
                Arc::clone(&self.active),
                timeline_tx,
                event_id.to_owned(),
                None,
                PollAction::Edit,
            );
        }
    }
}

async fn refuse_poll_action(timeline_tx: &mpsc::Sender<TimelineUpdate>, action: PollAction) {
    tracing::debug!(
        ?action,
        "demo: the room's power levels refuse this poll action"
    );
    drop(
        timeline_tx
            .send(TimelineUpdate::PollSendFailed(action))
            .await,
    );
}
