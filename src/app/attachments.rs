use std::sync::Arc;

use super::event::{AppEvent, AttachmentPicked};
use super::input::EventSender;
use super::show_toast;
use super::task_group::TaskGroup;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::{AttachmentKind, AttachmentView, Toast};
use crate::domain::media::{AttachmentPick, OutgoingAttachment, PickedAttachment};
use crate::domain::room::RoomId;
use crate::error::AppError;
use crate::ports::matrix::TimelinePort;
use crate::ports::media::MediaFilePort;
use crate::ports::output::AppOutputPort;
use crate::util::format_bytes;

pub(super) struct Attachments {
    media_files: Arc<dyn MediaFilePort>,
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    pending: Option<PickedAttachment>,
    room_id: Option<RoomId>,
    sending: bool,
}

impl Attachments {
    pub(super) fn new(
        media_files: Arc<dyn MediaFilePort>,
        output: Arc<dyn AppOutputPort>,
        events: EventSender,
    ) -> Self {
        Self {
            media_files,
            output,
            events,
            pending: None,
            room_id: None,
            sending: false,
        }
    }

    pub(super) fn pick(&mut self, group: &mut TaskGroup, room_id: RoomId, pick: AttachmentPick) {
        self.clear();
        let media_files = Arc::clone(&self.media_files);
        let events = self.events.clone();
        group.spawn(async move {
            let outcome = match media_files.pick_attachment(pick).await {
                Ok(None) => return,
                Ok(Some(picked)) => Ok(picked),
                Err(e) => {
                    tracing::warn!("failed to read the chosen attachment: {e}");
                    Err(UserMessage::new(UserMessageKind::AttachmentUnreadable))
                }
            };
            if events
                .send(AppEvent::AttachmentPicked(Box::new(AttachmentPicked {
                    room_id,
                    outcome,
                })))
                .is_err()
            {
                tracing::debug!("app event channel closed; dropping the picked attachment");
            }
        });
    }

    pub(super) fn adopt(&mut self, picked: AttachmentPicked, selected: Option<&RoomId>) {
        if selected != Some(&picked.room_id) {
            tracing::debug!("dropping an attachment picked for a room that is no longer selected");
            return;
        }
        match picked.outcome {
            Ok(attachment) => {
                self.room_id = Some(picked.room_id);
                self.publish(describe(&attachment, false, UserMessage::default()));
                self.pending = Some(attachment);
            }
            Err(failure) => show_toast(self.output.as_ref(), Toast::Error(failure)),
        }
    }

    pub(super) fn send(
        &mut self,
        group: &mut TaskGroup,
        timeline: Arc<dyn TimelinePort>,
        room_id: RoomId,
        caption: String,
        as_document: bool,
        reply_to: Option<String>,
    ) {
        if self.sending {
            return;
        }
        let Some(picked) = self.pending.clone() else {
            return;
        };
        if self.room_id.as_ref() != Some(&room_id) {
            return;
        }
        self.sending = true;
        self.publish(describe(&picked, true, UserMessage::default()));

        let attachment = OutgoingAttachment {
            picked,
            caption: (!caption.is_empty()).then_some(caption),
            as_document,
            reply_to,
        };
        let events = self.events.clone();
        group.spawn(async move {
            let outcome = timeline.send_attachment(&room_id, &attachment).await;
            let failure = match outcome {
                Ok(()) => None,
                Err(e) => {
                    tracing::warn!("failed to queue the attachment: {e}");
                    Some(failure_message(&e))
                }
            };
            if events
                .send(AppEvent::AttachmentSettled { room_id, failure })
                .is_err()
            {
                tracing::debug!("app event channel closed; dropping the attachment outcome");
            }
        });
    }

    pub(super) fn settle(&mut self, room_id: &RoomId, failure: Option<UserMessage>) {
        if self.room_id.as_ref() != Some(room_id) {
            return;
        }
        self.sending = false;
        let Some(failure) = failure else {
            self.clear();
            return;
        };
        let Some(picked) = self.pending.clone() else {
            return;
        };
        self.publish(describe(&picked, false, failure));
    }

    pub(super) fn clear(&mut self) {
        let had_state = self.pending.is_some() || self.room_id.is_some();
        self.pending = None;
        self.room_id = None;
        self.sending = false;
        if had_state {
            self.publish(AttachmentView::default());
        }
    }

    fn publish(&self, view: AttachmentView) {
        self.output
            .publish(Box::new(move |state| state.attachment = view));
    }
}

fn describe(picked: &PickedAttachment, sending: bool, error: UserMessage) -> AttachmentView {
    let (width, height) = picked.dimensions.unwrap_or((0, 0));
    AttachmentView {
        visible: true,
        filename: picked.filename.clone(),
        mimetype: picked.mimetype.clone(),
        size: picked.size,
        width,
        height,
        kind: attachment_kind(picked),
        duration: picked.duration,
        preview_path: picked.preview_path().cloned(),
        sending,
        error: error.kind,
        error_detail: error.detail,
    }
}

fn attachment_kind(picked: &PickedAttachment) -> AttachmentKind {
    if picked.is_video() {
        AttachmentKind::Video
    } else if picked.is_image() {
        AttachmentKind::Image
    } else {
        AttachmentKind::File
    }
}

fn failure_message(error: &AppError) -> UserMessage {
    match error {
        AppError::AttachmentTooLarge { limit } => {
            UserMessage::about(UserMessageKind::AttachmentTooLarge, &format_bytes(*limit))
        }
        AppError::Io(_) => UserMessage::new(UserMessageKind::AttachmentUnreadable),
        _ => UserMessage::new(UserMessageKind::SendAttachmentFailed),
    }
}
