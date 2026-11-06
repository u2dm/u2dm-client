use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::sleep;

use super::timeline_port::{opening_patch, send_patch, send_update};
use super::{ActiveRoom, DemoAuthed, ListKind, SharedActiveRoom, announce_lists};
use crate::adapters::demo::{attachments, data, polls, powers, reactions, receipts, timeline};
use crate::domain::member_name::MemberName;
use crate::domain::message::{MessageBody, RichText, SendState, TimelineMessage};
use crate::domain::poll::PollAction;
use crate::domain::timeline::{Arrivals, TimelineFocus, TimelinePatch, TimelineUpdate};

impl DemoAuthed {
    pub(super) fn spawn_poll_revocation(&self) {
        if !polls::scenario().permissions_get_revoked {
            return;
        }
        self.after_lag(polls::REVOKED_AFTER, |sink, lists| {
            if polls::revoke() {
                announce_lists(sink, lists, &[ListKind::Rooms]);
            }
        });
    }

    pub(super) fn spawn_power_change(&self) {
        self.after_lag(powers::SWITCH_AFTER, |sink, lists| {
            if powers::switch() {
                announce_lists(sink, lists, &[ListKind::Rooms]);
            }
        });
    }
}

fn spawn_late_reset(
    scenario: timeline::Scenario,
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    messages: Vec<TimelineMessage>,
) {
    tokio::spawn(async move {
        sleep(timeline::REPEATED_RESET_DELAY).await;
        let mut messages = messages;
        if scenario.repeat_drops_anchor {
            let keep = messages.len().saturating_sub(3);
            messages = messages.split_off(keep);
        }
        send_patch(&timeline_tx, opening_patch(scenario, messages)).await;
    });
}

fn spawn_row_churn(timeline_tx: mpsc::Sender<TimelineUpdate>, messages: Vec<TimelineMessage>) {
    tokio::spawn(async move {
        for round in 0..timeline::RESIZE_ROUNDS {
            sleep(timeline::RESIZE_INTERVAL).await;
            let Some(index) = messages.len().checked_sub(1) else {
                return;
            };
            let Some(message) = messages.get(index) else {
                return;
            };
            let mut message = message.clone();
            message.body = grown_body(&message.body, round);
            send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        }
    });
}

pub(super) fn spawn_scenario_tasks(
    active: &SharedActiveRoom,
    scenario: timeline::Scenario,
    focus: &TimelineFocus,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
    messages: &[TimelineMessage],
    reset_messages: Vec<TimelineMessage>,
) {
    if focus.is_live() && scenario.reset_repeats {
        spawn_late_reset(scenario, timeline_tx.clone(), reset_messages);
    }
    if scenario.rows_keep_resizing {
        spawn_row_churn(timeline_tx.clone(), messages.to_vec());
    }
    if scenario.message_arrives_late {
        spawn_late_append(Arc::clone(active), timeline_tx.clone());
    }
    if reactions::scenario().reactions_arrive_late {
        spawn_late_sets(
            timeline_tx.clone(),
            messages.to_vec(),
            reactions::reacted_indices(messages),
            reactions::LATE_INTERVAL,
        );
    }
    if polls::scenario().tallies_arrive_late {
        spawn_late_sets(
            timeline_tx.clone(),
            messages.to_vec(),
            polls::poll_indices(messages),
            polls::LATE_INTERVAL,
        );
    }
    if receipts::scenario().marks_arrive_late {
        spawn_late_sets(
            timeline_tx.clone(),
            messages.to_vec(),
            receipts::read_indices(messages),
            receipts::LATE_INTERVAL,
        );
    }
}

pub(super) fn spawn_seen(active: SharedActiveRoom, unique_id: String) {
    tokio::spawn(async move {
        sleep(receipts::SEEN_DELAY).await;
        let prepared = {
            let Ok(mut guard) = active.lock() else {
                return;
            };
            let Some(room) = guard.as_mut() else {
                return;
            };
            if !room
                .messages
                .iter()
                .any(|message| message.unique_id == unique_id)
            {
                return;
            }
            let reader = receipts::likely_reader(&room.messages);
            room.receipts.push(receipts::Receipt { unique_id, reader });
            let changed = receipts::stamp(&mut room.messages, &room.receipts);
            let patches: Vec<TimelinePatch> = changed
                .into_iter()
                .filter_map(|offset| {
                    let message = room.messages.get(offset)?.clone();
                    Some(TimelinePatch::Set {
                        index: room.prepended.saturating_add(offset),
                        message,
                    })
                })
                .collect();
            (room.timeline_tx.clone(), patches)
        };
        let (timeline_tx, patches) = prepared;
        if !patches.is_empty() {
            send_patch(&timeline_tx, TimelinePatch::Batch(patches)).await;
        }
    });
}

fn spawn_late_sets(
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    messages: Vec<TimelineMessage>,
    rows: Vec<usize>,
    interval: Duration,
) {
    tokio::spawn(async move {
        for index in rows {
            sleep(interval).await;
            let Some(message) = messages.get(index) else {
                return;
            };
            let message = message.clone();
            send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        }
    });
}

pub(super) fn spawn_poll_activity(
    active: &SharedActiveRoom,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
) {
    let scenario = polls::scenario();
    if scenario.members_keep_voting {
        let active = Arc::clone(active);
        let timeline_tx = timeline_tx.clone();
        tokio::spawn(async move {
            for round in 0..polls::LIVE_ROUNDS {
                sleep(polls::LIVE_INTERVAL).await;
                let voted = patch_newest_open_poll(&active, &timeline_tx, |message| {
                    polls::member_votes(message, round)
                });
                if !voted.await {
                    return;
                }
            }
        });
    }
    if scenario.newest_poll_ends {
        let active = Arc::clone(active);
        let timeline_tx = timeline_tx.clone();
        tokio::spawn(async move {
            sleep(polls::ENDS_AFTER).await;
            patch_newest_open_poll(&active, &timeline_tx, polls::close).await;
        });
    }
}

async fn patch_newest_open_poll(
    active: &SharedActiveRoom,
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
    change: impl FnOnce(&mut TimelineMessage) -> bool + Send,
) -> bool {
    let prepared = {
        let Ok(mut guard) = active.lock() else {
            return false;
        };
        let Some(room) = guard.as_mut() else {
            return false;
        };
        if !room.timeline_tx.same_channel(timeline_tx) {
            return false;
        }
        let Some(offset) = polls::newest_open_poll(&room.messages) else {
            return false;
        };
        let row = room.prepended.saturating_add(offset);
        let Some(message) = room.messages.get_mut(offset) else {
            return false;
        };
        if !change(message) {
            return false;
        }
        (row, message.clone())
    };
    let (index, message) = prepared;
    send_patch(timeline_tx, TimelinePatch::Set { index, message }).await;
    true
}

pub(super) fn spawn_poll_refusal(
    active: SharedActiveRoom,
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    event_id: String,
    undo: Option<polls::Undo>,
    action: PollAction,
) {
    tokio::spawn(async move {
        sleep(polls::REFUSAL_DELAY).await;
        if let Some(undo) = undo {
            let reverted = {
                let Ok(mut guard) = active.lock() else {
                    return;
                };
                let Some(room) = guard.as_mut() else {
                    return;
                };
                if !room.timeline_tx.same_channel(&timeline_tx) {
                    return;
                }
                let Some(offset) = room
                    .messages
                    .iter()
                    .position(|message| message.event_id.as_deref() == Some(event_id.as_str()))
                else {
                    return;
                };
                let row = room.prepended.saturating_add(offset);
                let Some(message) = room.messages.get_mut(offset) else {
                    return;
                };
                if !undo.apply(message) {
                    return;
                }
                (row, message.clone())
            };
            let (index, message) = reverted;
            send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        }
        drop(
            timeline_tx
                .send(TimelineUpdate::PollSendFailed(action))
                .await,
        );
    });
}

fn spawn_late_append(active: SharedActiveRoom, timeline_tx: mpsc::Sender<TimelineUpdate>) {
    tokio::spawn(async move {
        sleep(timeline::LATE_MESSAGE_DELAY).await;
        let mut message = data::own_message(
            9_000,
            "posted while the timeline was settling",
            &[],
            None,
            SendState::Sent,
        );
        message.is_own = false;
        "@sarah:matrix.org".clone_into(&mut message.sender);
        message.sender_name = MemberName::Unique("Sarah Chen".to_owned());
        let above_pending = active.lock().ok().and_then(|mut guard| {
            let room = guard
                .as_mut()
                .filter(|room| room.timeline_tx.same_channel(&timeline_tx))?;
            land_above_pending_sends(room, &message)
        });
        let arrivals = Arrivals::of([&message]);
        let patch = above_pending.unwrap_or(TimelinePatch::PushBack(message));
        let arrival = TimelineUpdate::Patch {
            patch: Box::new(patch),
            arrivals,
        };
        send_update(&timeline_tx, arrival).await;
    });
}

fn land_above_pending_sends(
    room: &mut ActiveRoom,
    arrival: &TimelineMessage,
) -> Option<TimelinePatch> {
    let pending_sends = room
        .messages
        .iter()
        .rev()
        .take_while(|message| message.local_id.is_some())
        .count();
    if pending_sends == 0 {
        return None;
    }
    let offset = room.messages.len().saturating_sub(pending_sends);
    room.messages.insert(offset, arrival.clone());
    Some(TimelinePatch::Insert {
        index: room.prepended.saturating_add(offset),
        message: arrival.clone(),
    })
}

fn grown_body(body: &MessageBody, round: usize) -> MessageBody {
    let padding = " and it keeps going".repeat(round % 4);
    match body {
        MessageBody::Text(text) if text.html().is_none() => {
            MessageBody::Text(RichText::plain(format!("{}{padding}", text.plain)))
        }
        other => other.clone(),
    }
}

pub(super) fn spawn_upload_progress(
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    index: usize,
    settled: TimelineMessage,
    total: u64,
) {
    tokio::spawn(async move {
        for step in 1..=attachments::UPLOAD_STEPS {
            sleep(attachments::UPLOAD_STEP_DELAY).await;
            let mut message = settled.clone();
            message.send_state = SendState::Uploading {
                sent: total * step / attachments::UPLOAD_STEPS,
                total,
            };
            send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        }
        sleep(attachments::UPLOAD_STEP_DELAY).await;
        send_patch(
            &timeline_tx,
            TimelinePatch::Set {
                index,
                message: settled,
            },
        )
        .await;
    });
}
