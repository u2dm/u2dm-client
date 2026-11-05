use std::collections::{BTreeMap, HashSet};
use std::mem;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use super::timeline_tasks::{
    spawn_poll_activity, spawn_poll_refusal, spawn_scenario_tasks, spawn_seen,
    spawn_upload_progress,
};
use super::{ActiveRoom, DemoAuthed, SharedActiveRoom, unavailable, with_room_facts};
use crate::adapters::body_emoji;
use crate::adapters::demo::identity::SendHold;
use crate::adapters::demo::{
    attachments, data, media, message_menu, polls, powers, reactions, receipts, room_info, source,
    timeline,
};
use crate::adapters::draft_size::{self, MediaDraft};
use crate::domain::emote::Emote;
use crate::domain::event_size::EventSize;
use crate::domain::image_pack::PackImage;
use crate::domain::media::OutgoingAttachment;
use crate::domain::message::{
    CustomEmoji, EditTarget, MessageBody, MessageEdit, MessagePreviewKind, ReplyInfo, RichText,
    SendState, TimelineMessage,
};
use crate::domain::poll::{PollAction, PollDraft};
use crate::domain::room::RoomId;
use crate::domain::timeline::{
    AudioLookup, AudioTrack, EventSource, JumpTarget, MessageReaders, PaginationDirection,
    PaginationOutcome, TimelineCommand, TimelineFocus, TimelinePatch, TimelineUpdate, locate_audio,
};
use crate::error::{AppError, Result};
use crate::ports::matrix::{AttachmentHandoff, TimelinePort};

struct Revision {
    body: MessageBody,
    body_emoji: BTreeMap<String, CustomEmoji>,
    mentions_room: bool,
    edited: bool,
}

impl Revision {
    fn restore(self, message: &mut TimelineMessage) {
        message.body = self.body;
        message.body_emoji = self.body_emoji;
        message.mentions_room = self.mentions_room;
        message.edited = self.edited;
    }
}

struct RevisedText {
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    row: usize,
    previous: Revision,
    message: TimelineMessage,
}

fn edit_names(target: &EditTarget, message: &TimelineMessage) -> bool {
    match target {
        EditTarget::Sent(event_id) => message.event_id.as_deref() == Some(event_id),
        EditTarget::Queued(local_id) => message.local_id.as_deref() == Some(local_id),
    }
}

fn revised_body(body: &MessageBody, text: &str, sent: RichText) -> Option<MessageBody> {
    match body {
        MessageBody::Text(_) => Some(MessageBody::Text(sent)),
        MessageBody::Image { meta, .. } => Some(MessageBody::Image {
            caption: revised_caption(text, sent),
            meta: meta.clone(),
        }),
        MessageBody::Video { meta, .. } => Some(MessageBody::Video {
            caption: revised_caption(text, sent),
            meta: meta.clone(),
        }),
        MessageBody::Audio { meta, .. } => Some(MessageBody::Audio {
            caption: revised_caption(text, sent),
            meta: meta.clone(),
        }),
        MessageBody::File { meta, .. } => Some(MessageBody::File {
            caption: revised_caption(text, sent),
            meta: meta.clone(),
        }),
        _ => None,
    }
}

fn revised_caption(text: &str, sent: RichText) -> Option<RichText> {
    (!text.is_empty()).then_some(sent)
}

struct OpenedWindow {
    all: Vec<TimelineMessage>,
    newer: usize,
    messages: Vec<TimelineMessage>,
    reset_messages: Vec<TimelineMessage>,
}

impl DemoAuthed {
    pub(super) fn room_history(&self, room_id: &RoomId) -> Vec<TimelineMessage> {
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

    pub(super) fn replay_live_window(&self) {
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

    fn send_state_in(&self, room_id: &RoomId) -> SendState {
        if self.holds_sends_in(room_id) {
            SendState::Failed
        } else {
            outgoing_send_state()
        }
    }

    fn holds_sends_in(&self, room_id: &RoomId) -> bool {
        let unverified_here =
            data::room_is_encrypted(room_id) && self.encryption.holds_encrypted_sends();
        unverified_here || matches!(self.identity.send_hold(room_id), SendHold::VerificationLost)
    }

    fn keep_own_send(&self, room_id: &RoomId, message: &TimelineMessage) {
        if let Ok(mut own_sends) = self.own_sends.lock() {
            own_sends
                .entry(room_id.clone())
                .or_default()
                .push(message.clone());
        }
    }

    fn settle_own_send(&self, room_id: &RoomId, local_id: &str, resend: bool) {
        let Ok(mut own_sends) = self.own_sends.lock() else {
            return;
        };
        let Some(sent) = own_sends.get_mut(room_id) else {
            return;
        };
        if resend {
            if let Some(message) = sent
                .iter_mut()
                .find(|m| m.local_id.as_deref() == Some(local_id))
            {
                mark_sent(message);
            }
        } else {
            sent.retain(|m| m.local_id.as_deref() != Some(local_id));
        }
    }

    fn keep_own_edit(&self, room_id: &RoomId, revised: &TimelineMessage) {
        let Ok(mut own_sends) = self.own_sends.lock() else {
            return;
        };
        let Some(sent) = own_sends
            .get_mut(room_id)
            .and_then(|sent| sent.iter_mut().find(|m| m.unique_id == revised.unique_id))
        else {
            return;
        };
        sent.body = revised.body.clone();
        sent.body_emoji = revised.body_emoji.clone();
        sent.mentions_room = revised.mentions_room;
        sent.edited = revised.edited;
    }

    fn revise_own_text(&self, room_id: &RoomId, edit: &MessageEdit) -> Option<RevisedText> {
        self.with_active(room_id, |active| {
            let offset = active
                .messages
                .iter()
                .position(|message| edit_names(&edit.target, message))?;
            let row = active.prepended.saturating_add(offset);
            let message = active.messages.get_mut(offset)?;
            message.editable_text()?;
            let emotes = with_target_emotes(&edit.emotes, message.body.html());
            let sent = data::sent_text(&edit.body, &emotes);
            let revised = revised_body(&message.body, &edit.body, sent.text)?;
            let revised_emoji = media::emoji_states(&revised);
            let previous = Revision {
                body: mem::replace(&mut message.body, revised),
                body_emoji: mem::replace(&mut message.body_emoji, revised_emoji),
                mentions_room: message.mentions_room,
                edited: message.edited,
            };
            message.mentions_room = sent.mentions_room;
            message.edited |= edit.target.event_id().is_some();
            if !timeline::scenario().edits_fail {
                self.keep_own_edit(room_id, message);
            }
            Some(RevisedText {
                timeline_tx: active.timeline_tx.clone(),
                row,
                previous,
                message: message.clone(),
            })
        })
    }

    fn with_active<T>(
        &self,
        room_id: &RoomId,
        work: impl FnOnce(&mut ActiveRoom) -> Option<T>,
    ) -> Option<T> {
        let mut guard = self.active.lock().ok()?;
        guard
            .as_mut()
            .filter(|active| &active.room_id == room_id)
            .and_then(work)
    }

    fn append_own(
        &self,
        room_id: &RoomId,
        build: impl FnOnce(&ActiveRoom) -> TimelineMessage,
    ) -> Option<(mpsc::Sender<TimelineUpdate>, usize, TimelineMessage)> {
        self.with_active(room_id, |active| {
            let message = build(active);
            self.keep_own_send(room_id, &message);
            if !active.live {
                return None;
            }
            active.messages.push(message.clone());
            let index = active.messages.len() - 1;
            Some((active.timeline_tx.clone(), index, message))
        })
    }

    fn reply_in_room(&self, active: &ActiveRoom, event_id: &str) -> Option<ReplyInfo> {
        reply_info(&active.messages, event_id)
            .or_else(|| reply_info(&self.room_history(&active.room_id), event_id))
    }

    async fn open_window(
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

    fn remove_from_window(
        &self,
        room_id: &RoomId,
        event_id: &str,
        restamp_replies: bool,
    ) -> Option<RemovedMessage> {
        self.with_active(room_id, |active| {
            let offset = active
                .messages
                .iter()
                .position(|message| message.event_id.as_deref() == Some(event_id))?;
            let message = active.messages.remove(offset);
            let mut replies = Vec::new();
            if restamp_replies {
                let deleted = HashSet::from([event_id.to_owned()]);
                for (index, reply) in active.messages.iter_mut().enumerate() {
                    if stamp_deleted_reply(reply, &deleted) {
                        replies.push((active.prepended.saturating_add(index), reply.clone()));
                    }
                }
            }
            Some(RemovedMessage {
                timeline_tx: active.timeline_tx.clone(),
                offset,
                row: active.prepended.saturating_add(offset),
                message,
                replies,
            })
        })
    }

    async fn patch_queued_send(
        &self,
        room_id: &RoomId,
        local_id: &str,
        resend: bool,
    ) -> Result<()> {
        let prepared = {
            let Ok(mut guard) = self.active.lock() else {
                return Err(unavailable("the demo timeline"));
            };
            let Some(active) = guard.as_mut() else {
                return Err(unavailable("the demo timeline"));
            };
            if &active.room_id != room_id {
                return Err(unavailable("the demo timeline"));
            }
            let index = active
                .messages
                .iter()
                .position(|m| m.local_id.as_deref() == Some(local_id))
                .ok_or_else(|| AppError::Other(format!("no queued send for {local_id}")))?;
            let patch = if resend {
                let Some(message) = active.messages.get_mut(index) else {
                    return Err(unavailable("the demo timeline"));
                };
                mark_sent(message);
                TimelinePatch::Set {
                    index,
                    message: message.clone(),
                }
            } else {
                active.messages.remove(index);
                TimelinePatch::Remove { index }
            };
            self.settle_own_send(room_id, local_id, resend);
            (active.timeline_tx.clone(), patch)
        };
        let (timeline_tx, patch) = prepared;
        send_patch(&timeline_tx, patch).await;
        Ok(())
    }

    async fn append_own_message(
        &self,
        room_id: &RoomId,
        body: &str,
        emotes: &[Emote],
        in_reply_to: Option<&str>,
    ) {
        let Some((timeline_tx, _, message)) = self.append_own(room_id, |active| {
            let reply = in_reply_to.and_then(|event_id| self.reply_in_room(active, event_id));
            data::own_message(
                self.sent.fetch_add(1, Ordering::Relaxed),
                body,
                emotes,
                reply,
                self.send_state_in(room_id),
            )
        }) else {
            return;
        };

        let seen = receipts::scenario().member_sees_sends && message.event_id.is_some();
        let unique_id = message.unique_id.clone();
        send_patch(&timeline_tx, TimelinePatch::PushBack(message)).await;
        if seen {
            spawn_seen(Arc::clone(&self.active), unique_id);
        }
    }

    async fn append_own_poll(&self, room_id: &RoomId, draft: &PollDraft) {
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

    pub(super) async fn append_own_sticker(
        &self,
        room_id: &RoomId,
        image: &PackImage,
        in_reply_to: Option<&str>,
    ) {
        let Some((timeline_tx, _, message)) = self.append_own(room_id, |active| {
            let reply = in_reply_to.and_then(|event_id| self.reply_in_room(active, event_id));
            data::own_sticker(self.sent.fetch_add(1, Ordering::Relaxed), image, reply)
        }) else {
            return;
        };

        send_patch(&timeline_tx, TimelinePatch::PushBack(message)).await;
    }

    pub(super) async fn post_own_row(&self, room_id: &RoomId, message: TimelineMessage) {
        self.keep_own_send(room_id, &message);
        let timeline_tx = {
            let Ok(mut guard) = self.active.lock() else {
                return;
            };
            let Some(active) = guard
                .as_mut()
                .filter(|active| &active.room_id == room_id && active.live)
            else {
                return;
            };
            active.messages.push(message.clone());
            active.timeline_tx.clone()
        };
        send_patch(&timeline_tx, TimelinePatch::PushBack(message)).await;
    }

    fn timeline_sender(&self, room_id: &RoomId) -> Option<mpsc::Sender<TimelineUpdate>> {
        let guard = self.active.lock().ok()?;
        let active = guard.as_ref()?;
        (&active.room_id == room_id).then(|| active.timeline_tx.clone())
    }

    async fn append_own_attachment(
        &self,
        room_id: &RoomId,
        attachment: &OutgoingAttachment,
        opening: SendState,
    ) -> Option<(usize, TimelineMessage)> {
        let (timeline_tx, index, settled) = self.append_own(room_id, |active| {
            let reply = attachment
                .reply_to
                .as_deref()
                .and_then(|event_id| self.reply_in_room(active, event_id));
            let settled =
                data::own_attachment(self.sent.fetch_add(1, Ordering::Relaxed), attachment, reply);
            remember_sent_media(&settled, attachment);
            settled
        })?;

        let mut opening_echo = settled.clone();
        opening_echo.send_state = opening;
        send_patch(&timeline_tx, TimelinePatch::PushBack(opening_echo)).await;
        Some((index, settled))
    }

    async fn toggle_reaction(&self, event_id: &str, key: &str) {
        if reactions::scenario().toggle_has_no_echo {
            tracing::debug!(event_id, "demo: swallowing a reaction toggle");
            return;
        }
        let prepared = {
            let Ok(mut guard) = self.active.lock() else {
                return;
            };
            let Some(active) = guard.as_mut() else {
                return;
            };
            let Some(offset) = active
                .messages
                .iter()
                .position(|message| message.event_id.as_deref() == Some(event_id))
            else {
                return;
            };
            let row = active.prepended.saturating_add(offset);
            let Some(message) = active.messages.get_mut(offset) else {
                return;
            };
            let own = reactions::own_send(message, key);
            if !powers::messages().allows_toggle(own) {
                tracing::info!(
                    event_id,
                    ?own,
                    "demo: the power levels refuse this reaction toggle"
                );
                return;
            }
            reactions::toggle(message, key, data::own_user());
            (active.timeline_tx.clone(), row, message.clone())
        };
        let (timeline_tx, index, message) = prepared;

        send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
    }

    async fn run_command(
        &self,
        command: TimelineCommand,
        timeline_tx: &mpsc::Sender<TimelineUpdate>,
    ) -> Option<PaginationDirection> {
        match command {
            TimelineCommand::PaginateBackwards => return Some(PaginationDirection::Backwards),
            TimelineCommand::PaginateForwards => return Some(PaginationDirection::Forwards),
            TimelineCommand::MarkRead => {}
            TimelineCommand::JumpTo(event_id) => {
                let target = self.loaded_row_of(&event_id);
                drop(
                    timeline_tx
                        .send(TimelineUpdate::JumpOutcome { event_id, target })
                        .await,
                );
            }
            TimelineCommand::ToggleReaction { event_id, key, .. } => {
                self.toggle_reaction(&event_id, &key).await;
            }
            TimelineCommand::VotePoll {
                event_id,
                answer_id,
            } => self.vote_poll(&event_id, &answer_id, timeline_tx).await,
            TimelineCommand::EndPoll { event_id } => self.end_poll(&event_id, timeline_tx).await,
            TimelineCommand::EditPoll { event_id, draft } => {
                self.edit_poll(&event_id, &draft, timeline_tx).await;
            }
            TimelineCommand::LocateAudio { request, lookup } => {
                let track = self.locate_audio(&lookup);
                drop(
                    timeline_tx
                        .send(TimelineUpdate::AudioLocated {
                            request,
                            track: track.map(Box::new),
                        })
                        .await,
                );
            }
            TimelineCommand::LocateSource { request, event_id } => {
                let source = self.locate_source(&event_id);
                drop(
                    timeline_tx
                        .send(TimelineUpdate::SourceLocated {
                            request,
                            source: source.map(Box::new),
                        })
                        .await,
                );
            }
            TimelineCommand::LocateReaders { request, event_id } => {
                let readers = self.locate_readers(&event_id);
                drop(
                    timeline_tx
                        .send(TimelineUpdate::ReadersLocated {
                            request,
                            readers: readers.map(Box::new),
                        })
                        .await,
                );
            }
        }
        None
    }

    fn locate_readers(&self, event_id: &str) -> Option<MessageReaders> {
        let guard = self.active.lock().ok()?;
        let active = guard.as_ref()?;
        receipts::readers_of(&active.messages, &active.receipts, event_id)
    }

    fn locate_source(&self, event_id: &str) -> Option<EventSource> {
        if message_menu::scenario().source_unavailable {
            return None;
        }
        let guard = self.active.lock().ok()?;
        let active = guard.as_ref()?;
        let message = active
            .messages
            .iter()
            .find(|message| message.event_id.as_deref() == Some(event_id))?;
        source::event_source(&active.room_id, message)
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

    async fn vote_poll(
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

    async fn end_poll(&self, event_id: &str, timeline_tx: &mpsc::Sender<TimelineUpdate>) {
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

    fn active_room_id(&self) -> Option<RoomId> {
        self.active
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|active| active.room_id.clone()))
    }

    async fn edit_poll(
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

    fn loaded_row_of(&self, event_id: &str) -> JumpTarget {
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

    fn locate_audio(&self, lookup: &AudioLookup) -> Option<AudioTrack> {
        let guard = self.active.lock().ok()?;
        let active = guard.as_ref()?;
        locate_audio(lookup, active.messages.iter().cloned())
    }

    async fn run_timeline(
        &self,
        room_id: &RoomId,
        focus: TimelineFocus,
        timeline_tx: mpsc::Sender<TimelineUpdate>,
        mut cmd_rx: mpsc::UnboundedReceiver<TimelineCommand>,
    ) -> Result<()> {
        let scenario = timeline::scenario();
        if scenario.resolving_unread && focus.opens_at_read_position() {
            drop(timeline_tx.send(TimelineUpdate::ResolvingUnread).await);
        }
        if scenario.unread_boundary_is_unresolved && focus.opens_at_read_position() {
            drop(timeline_tx.send(TimelineUpdate::UnreadUnresolved).await);
        }
        if scenario.reset_is_slow {
            sleep(timeline::SLOW_RESET_DELAY).await;
        }
        let Some(OpenedWindow {
            all,
            mut newer,
            messages,
            reset_messages,
        }) = self
            .open_window(room_id, &focus, scenario, &timeline_tx)
            .await?
        else {
            return Ok(());
        };

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

        spawn_scenario_tasks(
            &self.active,
            scenario,
            &focus,
            &timeline_tx,
            &messages,
            reset_messages,
        );
        spawn_poll_activity(&self.active, &timeline_tx);
        self.spawn_poll_revocation();
        self.spawn_power_change();

        let mut history = 0_u64;
        while let Some(command) = cmd_rx.recv().await {
            let Some(direction) = self.run_command(command, &timeline_tx).await else {
                continue;
            };
            let mut hit_end = true;
            if scenario.pagination_returns_history
                && matches!(direction, PaginationDirection::Backwards)
            {
                history += 1;
                hit_end = history > 2;
                let page = older_history(history, &messages);
                if let Ok(mut guard) = self.active.lock()
                    && let Some(active) = guard.as_mut()
                {
                    active.prepended = active.prepended.saturating_add(page.len());
                }
                let page = page.into_iter().map(TimelinePatch::PushFront).collect();
                send_patch(&timeline_tx, TimelinePatch::Batch(page)).await;
            }
            if matches!(direction, PaginationDirection::Forwards) {
                let page = newer_page(&all, &mut newer);
                hit_end = page.is_empty();
                self.append_newer(page, &timeline_tx).await;
            }
            let update = TimelineUpdate::Pagination {
                direction,
                outcome: PaginationOutcome::Completed { hit_end },
            };
            if timeline_tx.send(update).await.is_err() {
                break;
            }
        }

        Ok(())
    }
}

fn reset_messages(messages: &[TimelineMessage]) -> Vec<TimelineMessage> {
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

fn with_target_emotes(picked: &[Emote], target_html: Option<&str>) -> Vec<Emote> {
    let mut emotes = target_html.map(body_emoji::emotes_in).unwrap_or_default();
    for emote in picked {
        if !emotes.iter().any(|known| known.token == emote.token) {
            emotes.push(emote.clone());
        }
    }
    emotes
}

fn outgoing_send_state() -> SendState {
    if timeline::scenario().sends_fail {
        SendState::Failed
    } else if receipts::scenario().sends_stay_pending {
        SendState::Sending
    } else {
        SendState::Sent
    }
}

fn mark_sent(message: &mut TimelineMessage) {
    message.send_state = SendState::Sent;
    message.event_id = Some(message.unique_id.clone());
    message.local_id = None;
}

fn remember_sent_media(settled: &TimelineMessage, attachment: &OutgoingAttachment) {
    if let Some(event_id) = settled.event_id.as_deref()
        && !attachment.as_document
    {
        if let Some(preview) = attachment.picked.preview_path() {
            attachments::remember_preview(event_id, preview);
        }
        if attachment.picked.is_audio() {
            attachments::remember_sent_audio(event_id, &attachment.picked.path);
        }
    }
}

#[async_trait]
impl TimelinePort for DemoAuthed {
    async fn subscribe_timeline(
        &self,
        room_id: &RoomId,
        focus: TimelineFocus,
        timeline_tx: mpsc::Sender<TimelineUpdate>,
        cmd_rx: mpsc::UnboundedReceiver<TimelineCommand>,
        close: CancellationToken,
    ) -> Result<()> {
        let subscription = self.run_timeline(room_id, focus, timeline_tx, cmd_rx);
        close
            .run_until_cancelled(subscription)
            .await
            .unwrap_or(Ok(()))
    }

    async fn send_text(&self, room_id: &RoomId, body: &str, emotes: &[Emote]) -> Result<()> {
        powers::refuse_unless(powers::messages().send_messages, "sending messages")?;
        if timeline::scenario().sends_are_refused {
            return Err(unavailable("sending messages"));
        }
        with_room_facts(room_id, |facts| draft_size::message(body, emotes, facts)).checked()?;
        self.append_own_message(room_id, body, emotes, None).await;
        Ok(())
    }

    async fn send_reply(
        &self,
        room_id: &RoomId,
        body: &str,
        emotes: &[Emote],
        in_reply_to: &str,
    ) -> Result<()> {
        powers::refuse_unless(powers::messages().send_messages, "sending replies")?;
        if timeline::scenario().sends_are_refused {
            return Err(unavailable("sending replies"));
        }
        with_room_facts(room_id, |facts| {
            draft_size::reply(body, emotes, in_reply_to, facts)
        })
        .checked()?;
        self.append_own_message(room_id, body, emotes, Some(in_reply_to))
            .await;
        Ok(())
    }

    async fn edit_message(&self, room_id: &RoomId, edit: &MessageEdit) -> Result<()> {
        powers::refuse_unless(powers::messages().send_messages, "editing messages")?;
        if timeline::scenario().edits_are_refused {
            return Err(unavailable("editing messages"));
        }
        with_room_facts(room_id, |facts| draft_size::edit(edit, facts)).checked()?;
        let RevisedText {
            timeline_tx,
            row,
            previous,
            message,
        } = self.revise_own_text(room_id, edit).ok_or_else(|| {
            AppError::Other(
                "demo mode edits only an own text or caption in the open window".to_owned(),
            )
        })?;
        send_patch(
            &timeline_tx,
            TimelinePatch::Set {
                index: row,
                message,
            },
        )
        .await;
        if timeline::scenario().edits_fail && edit.target.event_id().is_some() {
            spawn_edit_refusal(
                Arc::clone(&self.active),
                timeline_tx,
                edit.clone(),
                previous,
            );
        }
        Ok(())
    }

    async fn send_poll(&self, room_id: &RoomId, draft: &PollDraft) -> Result<()> {
        powers::refuse_unless(powers::polls().start, "starting polls")?;
        if timeline::scenario().sends_are_refused {
            return Err(unavailable("sending polls"));
        }
        with_room_facts(room_id, |facts| draft_size::poll_start(draft, facts)).checked()?;
        self.append_own_poll(room_id, draft).await;
        Ok(())
    }

    async fn resend(&self, room_id: &RoomId, local_id: &str) -> Result<()> {
        if self.holds_sends_in(room_id) {
            tracing::debug!(%room_id, local_id, "demo: the retried send is held again");
            return Ok(());
        }
        self.patch_queued_send(room_id, local_id, true).await
    }

    async fn discard_send(&self, room_id: &RoomId, local_id: &str) -> Result<()> {
        self.patch_queued_send(room_id, local_id, false).await
    }

    async fn delete_message(&self, room_id: &RoomId, event_id: &str) -> Result<()> {
        message_menu::pause().await;
        if self
            .deleted
            .lock()
            .is_ok_and(|deleted| deleted.contains(event_id))
        {
            tracing::debug!(event_id, "demo: the message is already deleted");
            return Ok(());
        }
        let own = self
            .room_history(room_id)
            .iter()
            .find(|message| message.event_id.as_deref() == Some(event_id))
            .map(|message| message.is_own)
            .ok_or_else(|| AppError::Other(format!("{event_id} is not in {room_id}")))?;
        let permissions = powers::messages();
        let allowed = if own {
            permissions.delete_own
        } else {
            permissions.delete_others
        };
        if !allowed {
            return Err(unavailable("deleting this message"));
        }
        if message_menu::scenario().delete_refused {
            return Err(unavailable("deleting messages"));
        }
        tracing::debug!(%room_id, event_id, "demo: deleting a message");
        let fails = message_menu::scenario().delete_fails;
        if !fails && let Ok(mut deleted) = self.deleted.lock() {
            deleted.insert(event_id.to_owned());
        }
        let Some(removed) = self.remove_from_window(room_id, event_id, !fails) else {
            return Ok(());
        };
        let timeline_tx = removed.timeline_tx.clone();
        send_patch(&timeline_tx, TimelinePatch::Remove { index: removed.row }).await;
        for (index, message) in removed.replies.clone() {
            send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        }
        if fails {
            spawn_deletion_refusal(Arc::clone(&self.active), removed);
        } else {
            self.change_board(room_id, |_| {});
        }
        Ok(())
    }

    async fn send_attachment(
        &self,
        room_id: &RoomId,
        attachment: &OutgoingAttachment,
        abandon: CancellationToken,
    ) -> Result<AttachmentHandoff> {
        powers::refuse_unless(powers::messages().send_messages, "sending attachments")?;
        if attachments::scenario().refuses_size {
            return Err(AppError::AttachmentTooLarge {
                limit: attachments::upload_limit(),
            });
        }
        let content_type = if attachment.as_document {
            mime::APPLICATION_OCTET_STREAM.as_ref()
        } else {
            attachment.picked.mimetype.as_str()
        };
        let media = MediaDraft {
            attachment,
            content_type,
            thumbnail: false,
        };
        with_room_facts(room_id, |facts| draft_size::media(&media, facts)).checked()?;
        if attachments::scenario().send_fails {
            tokio::select! {
                biased;
                () = abandon.cancelled() => return Ok(AttachmentHandoff::Abandoned),
                () = attachments::pause_upload() => {}
            }
            return Err(unavailable("sending attachments"));
        }

        let total = attachment.picked.size;
        let uploads_slowly = attachments::scenario().upload_is_slow;
        let opening = if uploads_slowly {
            SendState::Uploading { sent: 0, total }
        } else {
            SendState::Sent
        };
        let Some((index, settled)) = self
            .append_own_attachment(room_id, attachment, opening)
            .await
        else {
            return Ok(AttachmentHandoff::Queued);
        };
        if uploads_slowly && let Some(timeline_tx) = self.timeline_sender(room_id) {
            spawn_upload_progress(timeline_tx, index, settled, total);
        }
        Ok(AttachmentHandoff::Queued)
    }
}

fn reply_info(messages: &[TimelineMessage], event_id: &str) -> Option<ReplyInfo> {
    messages
        .iter()
        .find(|message| message.event_id.as_deref() == Some(event_id))
        .map(|message| ReplyInfo {
            event_id: event_id.to_owned(),
            sender: data::sender_of(message),
            kind: message.body.preview_kind(),
            body: data::body_preview(&message.body),
        })
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

pub(super) fn opening_patch(
    scenario: timeline::Scenario,
    messages: Vec<TimelineMessage>,
) -> TimelinePatch {
    let reset = TimelinePatch::Reset(messages);
    if scenario.reset_is_batched {
        return TimelinePatch::Batch(vec![reset]);
    }
    reset
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

fn spawn_edit_refusal(
    active: SharedActiveRoom,
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    edit: MessageEdit,
    previous: Revision,
) {
    tokio::spawn(async move {
        sleep(timeline::EDIT_REFUSAL_DELAY).await;
        let reverted = active.lock().ok().and_then(|mut guard| {
            let room = guard
                .as_mut()
                .filter(|room| room.timeline_tx.same_channel(&timeline_tx))?;
            let offset = room
                .messages
                .iter()
                .position(|message| edit_names(&edit.target, message))?;
            let row = room.prepended.saturating_add(offset);
            let message = room.messages.get_mut(offset)?;
            if message.editable_text() != Some(edit.body.as_str()) {
                return None;
            }
            previous.restore(message);
            Some((row, message.clone()))
        });
        if let Some((index, message)) = reverted {
            send_patch(&timeline_tx, TimelinePatch::Set { index, message }).await;
        }
        let unsaved = MessageEdit {
            original: None,
            ..edit
        };
        drop(timeline_tx.send(TimelineUpdate::EditUnsaved(unsaved)).await);
    });
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

struct RemovedMessage {
    timeline_tx: mpsc::Sender<TimelineUpdate>,
    offset: usize,
    row: usize,
    message: TimelineMessage,
    replies: Vec<(usize, TimelineMessage)>,
}

fn stamp_deleted_reply(message: &mut TimelineMessage, deleted: &HashSet<String>) -> bool {
    let Some(reply) = message.reply.as_mut() else {
        return false;
    };
    if !deleted.contains(&reply.event_id) || reply.kind == MessagePreviewKind::Deleted {
        return false;
    }
    reply.kind = MessagePreviewKind::Deleted;
    reply.body = RichText::default();
    true
}

fn spawn_deletion_refusal(active: SharedActiveRoom, removed: RemovedMessage) {
    tokio::spawn(async move {
        sleep(message_menu::REFUSAL_DELAY).await;
        let restored = {
            let Ok(mut guard) = active.lock() else {
                return;
            };
            let Some(room) = guard.as_mut() else {
                return;
            };
            if !room.timeline_tx.same_channel(&removed.timeline_tx) {
                return;
            }
            let offset = removed.offset.min(room.messages.len());
            room.messages.insert(offset, removed.message.clone());
            room.prepended.saturating_add(offset)
        };
        send_patch(
            &removed.timeline_tx,
            TimelinePatch::Insert {
                index: restored,
                message: removed.message,
            },
        )
        .await;
        drop(removed.timeline_tx.send(TimelineUpdate::DeleteFailed).await);
    });
}

pub(super) async fn send_patch(timeline_tx: &mpsc::Sender<TimelineUpdate>, patch: TimelinePatch) {
    send_update(timeline_tx, TimelineUpdate::patch(patch)).await;
}

pub(super) async fn send_update(
    timeline_tx: &mpsc::Sender<TimelineUpdate>,
    update: TimelineUpdate,
) {
    if let Err(e) = timeline_tx.send(update).await {
        tracing::debug!("demo timeline receiver closed: {e}");
    }
}
