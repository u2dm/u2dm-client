mod deletions;
mod edits;
mod poll_ops;
mod sends;
mod window;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use self::deletions::spawn_deletion_refusal;
use self::edits::{RevisedText, spawn_edit_refusal};
use self::sends::Settle;
pub(super) use self::sends::{send_patch, send_update};
pub(super) use self::window::opening_patch;
use self::window::{OpenedWindow, Paging};
use super::timeline_tasks::{spawn_poll_activity, spawn_scenario_tasks, spawn_upload_progress};
use super::{ActiveRoom, DemoAuthed, unavailable, unavailable_if, with_room_facts};
use crate::adapters::demo::{
    attachments, data, message_menu, powers, reactions, receipts, source, timeline,
};
use crate::adapters::draft_size::{self, MediaDraft};
use crate::domain::emote::Emote;
use crate::domain::media::OutgoingAttachment;
use crate::domain::message::{MessageEdit, SendState};
use crate::domain::poll::PollDraft;
use crate::domain::room::RoomId;
use crate::domain::timeline::{
    AudioLookup, AudioTrack, EventSource, MessageReaders, PaginationDirection, PaginationOutcome,
    TimelineCommand, TimelineFocus, TimelinePatch, TimelineUpdate, locate_audio,
};
use crate::error::{AppError, Result};
use crate::ports::matrix::{AttachmentHandoff, TimelinePort};

impl DemoAuthed {
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
            TimelineCommand::MarkRead(visibility) => {
                tracing::debug!(?visibility, "demo: a read receipt goes out");
            }
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

    fn active_room_id(&self) -> Option<RoomId> {
        self.active
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|active| active.room_id.clone()))
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
            newer,
            messages,
            reset_messages,
        }) = self
            .open_window(room_id, &focus, scenario, &timeline_tx)
            .await?
        else {
            return Ok(());
        };
        self.announce_opening(&focus, scenario, &timeline_tx).await;

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

        let mut paging = Paging {
            returns_history: scenario.pagination_returns_history,
            history: 0,
            messages,
            all,
            newer,
        };
        while let Some(command) = cmd_rx.recv().await {
            let Some(direction) = self.run_command(command, &timeline_tx).await else {
                continue;
            };
            let hit_end = self.page_in(direction, &mut paging, &timeline_tx).await;
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
        unavailable_if(timeline::scenario().sends_are_refused, "sending messages")?;
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
        unavailable_if(timeline::scenario().sends_are_refused, "sending replies")?;
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
        unavailable_if(timeline::scenario().edits_are_refused, "editing messages")?;
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
        unavailable_if(timeline::scenario().sends_are_refused, "sending polls")?;
        with_room_facts(room_id, |facts| draft_size::poll_start(draft, facts)).checked()?;
        self.append_own_poll(room_id, draft).await;
        Ok(())
    }

    async fn resend(&self, room_id: &RoomId, local_id: &str) -> Result<()> {
        if self.holds_sends_in(room_id) {
            tracing::debug!(%room_id, local_id, "demo: the retried send is held again");
            return Ok(());
        }
        self.patch_queued_send(room_id, local_id, Settle::Resend)
            .await
    }

    async fn discard_send(&self, room_id: &RoomId, local_id: &str) -> Result<()> {
        self.patch_queued_send(room_id, local_id, Settle::Discard)
            .await
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
        unavailable_if(!allowed, "deleting this message")?;
        unavailable_if(message_menu::scenario().delete_refused, "deleting messages")?;
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
                limit: attachments::PRETEND_UPLOAD_LIMIT,
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
