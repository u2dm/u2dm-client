use super::Tx;
use crate::adapters::draft_size;
use crate::adapters::ui::emote;
use crate::adapters::ui::props::send_command;
use crate::adapters::ui::sending::SendingContext;
use crate::commands::ui::{MessageDraft, ReplyDraft, UiCommand};
use crate::domain::emote::ComposerText;
use crate::domain::event_size::{EventSize, Oversize};
use crate::domain::message::{EditKind, EditTarget, MessageEdit, TextRevision};
use crate::domain::poll::{ChoiceMode, PollDisclosure, PollDraft};
use crate::domain::room::RoomId;

pub enum DraftVerdict {
    Accepted,
    Incomplete,
    TooLong(Oversize),
}

impl DraftVerdict {
    pub fn accepted(&self) -> bool {
        matches!(self, Self::Accepted)
    }

    pub fn shorten_by(&self) -> i32 {
        match self {
            Self::TooLong(oversize) => i32::from(oversize.shorten_by()),
            Self::Accepted | Self::Incomplete => 0,
        }
    }
}

fn sent_if_it_fits(size: EventSize, send: impl FnOnce()) -> DraftVerdict {
    match size.checked() {
        Ok(_) => {
            send();
            DraftVerdict::Accepted
        }
        Err(oversize) => {
            tracing::info!(%oversize, "refusing a draft too large to send");
            DraftVerdict::TooLong(oversize)
        }
    }
}

pub fn send_message(
    tx: &Tx,
    room_id: String,
    body: String,
    reply_to: String,
    reply_sender: String,
    reply_preview: String,
) -> DraftVerdict {
    if room_id.is_empty() || body.is_empty() {
        return DraftVerdict::Incomplete;
    }
    let emotes = emote::bound_in(&body);
    let context = SendingContext::of(&room_id);
    let size = if reply_to.is_empty() {
        draft_size::message(&body, &emotes, &context.facts())
    } else {
        draft_size::reply(&body, &emotes, &reply_to, &context.facts())
    };
    let reply = (!reply_to.is_empty()).then_some(ReplyDraft {
        event_id: reply_to,
        sender: reply_sender,
        preview: reply_preview,
    });
    sent_if_it_fits(size, || {
        send_command(
            tx,
            UiCommand::SendMessage {
                room_id: RoomId::new(room_id),
                draft: MessageDraft {
                    body,
                    reply,
                    emotes,
                },
            },
        );
    })
}

pub struct EditedRow {
    pub event_id: String,
    pub local_id: String,
}

pub struct EditedText {
    pub caption: bool,
    pub restored: bool,
    pub original: String,
    pub body: String,
}

pub fn edit_message(tx: &Tx, room_id: String, row: EditedRow, text: EditedText) -> DraftVerdict {
    if room_id.is_empty() {
        return DraftVerdict::Incomplete;
    }
    let Some(target) = EditTarget::of(row.event_id, row.local_id) else {
        return DraftVerdict::Incomplete;
    };
    let kind = if text.caption {
        EditKind::Caption
    } else {
        EditKind::Message
    };
    let original = (!text.restored).then_some(text.original);
    match TextRevision::of(kind, original.as_deref(), text.body) {
        TextRevision::Blank => DraftVerdict::Incomplete,
        TextRevision::Unchanged => DraftVerdict::Accepted,
        TextRevision::Changed(body) => {
            let edit = MessageEdit {
                target,
                kind,
                emotes: emote::bound_in(&body),
                body,
                original,
            };
            let size = draft_size::edit(&edit, &SendingContext::of(&room_id).facts());
            sent_if_it_fits(size, || {
                send_command(
                    tx,
                    UiCommand::EditMessage {
                        room_id: RoomId::new(room_id),
                        edit,
                    },
                );
            })
        }
    }
}

pub fn send_attachment(
    tx: &Tx,
    room_id: String,
    caption: String,
    as_document: bool,
    reply_to: String,
) {
    if room_id.is_empty() {
        return;
    }
    send_command(
        tx,
        UiCommand::SendAttachment {
            room_id: RoomId::new(room_id),
            caption: ComposerText {
                emotes: emote::bound_in(&caption),
                text: caption,
            },
            as_document,
            reply_to: (!reply_to.is_empty()).then_some(reply_to),
        },
    );
}

fn poll_draft(
    question: String,
    answers: Vec<String>,
    multiple: bool,
    hide_results: bool,
) -> Option<PollDraft> {
    PollDraft::parse(
        question,
        answers,
        ChoiceMode::from_multiple(multiple),
        PollDisclosure::from_hidden(hide_results),
    )
    .inspect_err(|reason| tracing::debug!(?reason, "refusing a poll draft"))
    .ok()
}

pub fn send_poll(
    tx: &Tx,
    room_id: String,
    question: String,
    answers: Vec<String>,
    multiple: bool,
    hide_results: bool,
) -> DraftVerdict {
    if room_id.is_empty() {
        return DraftVerdict::Incomplete;
    }
    let Some(draft) = poll_draft(question, answers, multiple, hide_results) else {
        return DraftVerdict::Incomplete;
    };
    let size = draft_size::poll_start(&draft, &SendingContext::of(&room_id).facts());
    sent_if_it_fits(size, || {
        send_command(
            tx,
            UiCommand::SendPoll {
                room_id: RoomId::new(room_id),
                draft,
            },
        );
    })
}

pub fn edit_poll(
    tx: &Tx,
    event_id: String,
    question: String,
    answers: Vec<String>,
    multiple: bool,
    hide_results: bool,
) -> DraftVerdict {
    if event_id.is_empty() {
        return DraftVerdict::Incomplete;
    }
    let Some(draft) = poll_draft(question, answers, multiple, hide_results) else {
        return DraftVerdict::Incomplete;
    };
    let size = draft_size::poll_edit(&draft, &event_id, &SendingContext::selected().facts());
    sent_if_it_fits(size, || {
        send_command(tx, UiCommand::EditPoll { event_id, draft });
    })
}
