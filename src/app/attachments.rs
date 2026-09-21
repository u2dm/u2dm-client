use std::sync::Arc;

use super::event::{AppEvent, AttachmentPicked};
use super::input::EventSender;
use super::send_lanes::SendLanes;
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

struct Draft {
    pick: u64,
    room_id: RoomId,
    attachment: PickedAttachment,
    submission: Option<u64>,
}

impl Draft {
    fn is_sending(&self) -> bool {
        self.submission.is_some()
    }

    fn accepts_send(&self, room_id: &RoomId) -> bool {
        !self.is_sending() && &self.room_id == room_id
    }

    fn awaits(&self, submission: u64) -> bool {
        self.submission == Some(submission)
    }
}

pub(super) struct Attachments {
    media_files: Arc<dyn MediaFilePort>,
    output: Arc<dyn AppOutputPort>,
    events: EventSender,
    draft: Option<Draft>,
    picks: u64,
    submissions: u64,
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
            draft: None,
            picks: 0,
            submissions: 0,
        }
    }

    pub(super) fn pick(&mut self, group: &mut TaskGroup, room_id: RoomId, source: AttachmentPick) {
        self.clear();
        self.picks = self.picks.saturating_add(1);
        let pick = self.picks;
        let media_files = Arc::clone(&self.media_files);
        let events = self.events.clone();
        group.spawn(async move {
            let outcome = match media_files.pick_attachment(source).await {
                Ok(None) => return,
                Ok(Some(picked)) => Ok(picked),
                Err(e) => {
                    tracing::warn!("failed to read the chosen attachment: {e}");
                    Err(UserMessage::new(UserMessageKind::AttachmentUnreadable))
                }
            };
            if events
                .send(AppEvent::AttachmentPicked(Box::new(AttachmentPicked {
                    pick,
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
        if picked.pick != self.picks {
            tracing::debug!("dropping an attachment picked for a draft that was replaced");
            return;
        }
        if selected != Some(&picked.room_id) {
            tracing::debug!("dropping an attachment picked for a room that is no longer selected");
            return;
        }
        match picked.outcome {
            Ok(attachment) => {
                let draft = Draft {
                    pick: picked.pick,
                    room_id: picked.room_id,
                    attachment,
                    submission: None,
                };
                self.publish(describe(&draft, UserMessage::default()));
                self.draft = Some(draft);
            }
            Err(failure) => show_toast(self.output.as_ref(), Toast::Error(failure)),
        }
    }

    pub(super) fn send(
        &mut self,
        lanes: &mut SendLanes,
        timeline: Arc<dyn TimelinePort>,
        room_id: RoomId,
        caption: String,
        as_document: bool,
        reply_to: Option<String>,
    ) {
        self.submissions = self.submissions.saturating_add(1);
        let submission = self.submissions;
        let Some(draft) = self
            .draft
            .as_mut()
            .filter(|draft| draft.accepts_send(&room_id))
        else {
            return;
        };
        draft.submission = Some(submission);
        let attachment = OutgoingAttachment {
            picked: draft.attachment.clone(),
            caption: (!caption.is_empty()).then_some(caption),
            as_document,
            reply_to,
        };
        let sending = describe(draft, UserMessage::default());
        self.publish(sending);

        let events = self.events.clone();
        lanes.spawn(room_id.clone(), async move {
            let outcome = timeline.send_attachment(&room_id, &attachment).await;
            let failure = match outcome {
                Ok(()) => None,
                Err(e) => {
                    tracing::warn!("failed to queue the attachment: {e}");
                    Some(failure_message(&e))
                }
            };
            if events
                .send(AppEvent::AttachmentSettled {
                    submission,
                    failure,
                })
                .is_err()
            {
                tracing::debug!("app event channel closed; dropping the attachment outcome");
            }
        });
    }

    pub(super) fn settle(&mut self, submission: u64, failure: Option<UserMessage>) {
        let Some(draft) = self.draft.as_mut().filter(|draft| draft.awaits(submission)) else {
            tracing::debug!("dropping the outcome of an abandoned attachment send");
            return;
        };
        draft.submission = None;
        match failure {
            Some(failure) => {
                let failed = describe(draft, failure);
                self.publish(failed);
            }
            None => self.clear(),
        }
    }

    pub(super) fn clear(&mut self) {
        if self.draft.take().is_some() {
            self.publish(AttachmentView::default());
        }
    }

    fn publish(&self, view: AttachmentView) {
        self.output
            .publish(Box::new(move |state| state.attachment = view));
    }
}

fn describe(draft: &Draft, error: UserMessage) -> AttachmentView {
    let picked = &draft.attachment;
    let (width, height) = picked.dimensions.unwrap_or((0, 0));
    AttachmentView {
        pick: draft.pick,
        visible: true,
        filename: picked.filename.clone(),
        mimetype: picked.mimetype.clone(),
        size: picked.size,
        width,
        height,
        kind: attachment_kind(picked),
        duration: picked.duration,
        preview_path: picked.preview_path().cloned(),
        sending: draft.is_sending(),
        error: error.kind,
        error_detail: error.detail,
    }
}

fn attachment_kind(picked: &PickedAttachment) -> AttachmentKind {
    if picked.is_video() {
        AttachmentKind::Video
    } else if picked.is_audio() {
        AttachmentKind::Audio
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
