use serde::Deserialize;

use super::Rejected;
use crate::adapters::demo::probe::names;
use crate::commands::ui::UiCommand;
use crate::domain::emote::Emote;
use crate::domain::message::{EditKind, EditTarget, MessageEdit, TextRevision};
use crate::domain::poll::{ChoiceMode, PollDisclosure, PollDraft};
use crate::domain::room_creation::{RoomAccess, RoomDraft, RoomKind};

const PROBE_EMOTE_HEIGHT: &str = "32";

pub(super) struct CreateRoomOp {
    pub(super) name: String,
    pub(super) topic: String,
    pub(super) kind: String,
    pub(super) access: String,
    pub(super) address: String,
    pub(super) encrypted: bool,
    pub(super) in_space: bool,
}

pub(super) fn default_room_kind() -> String {
    names::room_kind(RoomKind::Room).to_owned()
}

pub(super) fn default_room_access() -> String {
    names::room_access(RoomAccess::InviteOnly).to_owned()
}

pub(super) fn encrypted_by_default() -> bool {
    true
}

pub(super) fn in_space_by_default() -> bool {
    true
}

pub(super) fn room_draft(draft: CreateRoomOp) -> Result<UiCommand, Rejected> {
    let kind = names::room_kind_named(&draft.kind).ok_or_else(|| {
        Rejected(format!(
            "{} is not a room kind; use room or space",
            draft.kind
        ))
    })?;
    let access = names::room_access_named(&draft.access).ok_or_else(|| {
        Rejected(format!(
            "{} is not a join rule; use invite-only, space-members or public",
            draft.access
        ))
    })?;
    Ok(UiCommand::CreateRoom(RoomDraft {
        kind,
        name: draft.name,
        topic: draft.topic,
        access,
        address: draft.address,
        encrypted: draft.encrypted,
        in_space: draft.in_space,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmoteDto {
    token: String,
    mxc: String,
    title: String,
    #[serde(default)]
    alt: Option<String>,
}

impl EmoteDto {
    pub(super) fn into_emote(self) -> Emote {
        Emote {
            token: self.token,
            mxc: self.mxc,
            title: self.title,
            alt: self.alt,
            height: Some(PROBE_EMOTE_HEIGHT.to_owned()),
            width: None,
        }
    }
}

pub(super) fn message_edit(
    target: Option<EditTarget>,
    caption: bool,
    body: String,
    original: Option<String>,
    emotes: Vec<Emote>,
) -> Result<MessageEdit, Rejected> {
    let target = target.ok_or_else(|| {
        Rejected("an edit needs the event_id or the local_id it edits".to_owned())
    })?;
    let kind = if caption {
        EditKind::Caption
    } else {
        EditKind::Message
    };
    match TextRevision::of(kind, original.as_deref(), body) {
        TextRevision::Blank => Err(Rejected("an edit to blank text is refused".to_owned())),
        TextRevision::Unchanged => Err(Rejected(
            "the edit restates the original, so nothing would be sent".to_owned(),
        )),
        TextRevision::Changed(body) => Ok(MessageEdit {
            target,
            kind,
            body,
            original,
            emotes,
        }),
    }
}

pub(super) fn poll_draft(
    question: String,
    answers: Vec<String>,
    multiple: bool,
    hide_results: bool,
) -> Result<PollDraft, Rejected> {
    PollDraft::parse(
        question,
        answers,
        ChoiceMode::from_multiple(multiple),
        PollDisclosure::from_hidden(hide_results),
    )
    .map_err(|reason| Rejected(format!("the poll draft is invalid: {reason:?}")))
}
