use std::fmt;

use serde::Deserialize;

use std::time::Duration;

use super::names;
use crate::adapters::demo::attachments;
use crate::adapters::ui::dump::Poke;
use crate::commands::ui::{MessageDraft, ReplyDraft, UiCommand};
use crate::domain::link::LauncherSafeUrl;
use crate::domain::media::AttachmentPick;
use crate::domain::message::{EditKind, EditTarget, MessageEdit, TextRevision};
use crate::domain::poll::{ChoiceMode, PollDisclosure, PollDraft};
use crate::domain::room::RoomId;
use crate::domain::sticker::PackId;

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProbeCommand {
    RetryRestore,
    CheckServer {
        homeserver: String,
    },
    LoginOauth,
    CancelOauth,
    BackToHomeserver,
    ReauthOauth,
    SelectSpace {
        #[serde(default)]
        space_id: Option<String>,
    },
    SelectDirect,
    SelectSubspace {
        #[serde(default)]
        subspace_id: Option<String>,
    },
    MoveSpace {
        from: usize,
        to: usize,
    },
    SelectRoom {
        room_id: String,
    },
    OpenSpaceIndex,
    CloseSpaceIndex,
    PageSpaceIndex,
    RetrySpaceIndex,
    JoinSpaceChild {
        room_id: String,
    },
    OpenSpaceChild {
        room_id: String,
    },
    OpenRoomInfo {
        #[serde(default)]
        room_id: Option<String>,
    },
    CloseRoomInfo,
    PageRoomMembers,
    RetryRoomMembers,
    FilterRoomMembers {
        query: String,
    },
    SetRoomNotify {
        #[serde(default)]
        room_id: Option<String>,
        mode: String,
    },
    LeaveRoom {
        #[serde(default)]
        room_id: Option<String>,
    },
    OpenRoomMenu {
        #[serde(default)]
        room_id: Option<String>,
    },
    CloseRoomMenu,
    MarkRoomRead {
        #[serde(default)]
        room_id: Option<String>,
    },
    CopyRoomLink {
        #[serde(default)]
        room_id: Option<String>,
    },
    SendMessage {
        #[serde(default)]
        room_id: Option<String>,
        body: String,
        #[serde(default)]
        reply_to: Option<String>,
        #[serde(default)]
        reply_sender: String,
        #[serde(default)]
        reply_preview: String,
    },
    EditMessage {
        #[serde(default)]
        room_id: Option<String>,
        #[serde(default)]
        event_id: String,
        #[serde(default)]
        local_id: String,
        #[serde(default)]
        caption: bool,
        body: String,
        #[serde(default)]
        original: Option<String>,
    },
    DismissUnsent {
        submission: i32,
    },
    SendPoll {
        #[serde(default)]
        room_id: Option<String>,
        question: String,
        #[serde(default)]
        answers: Vec<String>,
        #[serde(default)]
        multiple: bool,
        #[serde(default)]
        hide_results: bool,
    },
    EditPoll {
        event_id: String,
        question: String,
        #[serde(default)]
        answers: Vec<String>,
        #[serde(default)]
        multiple: bool,
        #[serde(default)]
        hide_results: bool,
    },
    SendSticker {
        #[serde(default)]
        room_id: Option<String>,
        pack: String,
        shortcode: String,
        #[serde(default)]
        reply_to: Option<String>,
    },
    PickAttachment {
        #[serde(default)]
        room_id: Option<String>,
        #[serde(default)]
        document: bool,
    },
    SendAttachment {
        #[serde(default)]
        room_id: Option<String>,
        #[serde(default)]
        caption: String,
        #[serde(default)]
        as_document: bool,
        #[serde(default)]
        reply_to: Option<String>,
    },
    CancelAttachment,
    PaginateBackwards {
        #[serde(default)]
        room_id: Option<String>,
        #[serde(default)]
        generation: Option<i32>,
    },
    PaginateForwards {
        #[serde(default)]
        room_id: Option<String>,
        #[serde(default)]
        generation: Option<i32>,
    },
    JumpToLatest {
        #[serde(default)]
        room_id: Option<String>,
        #[serde(default)]
        generation: Option<i32>,
    },
    JumpToEvent {
        event_id: String,
    },
    OpenPinned {
        event_id: String,
    },
    CopyMessageLink {
        event_id: String,
    },
    OpenEventSource {
        event_id: String,
    },
    CloseEventSource,
    PinMessage {
        event_id: String,
    },
    UnpinMessage {
        event_id: String,
    },
    DeleteMessage {
        event_id: String,
    },
    ToggleReaction {
        event_id: String,
        key: String,
    },
    VotePoll {
        event_id: String,
        answer_id: String,
    },
    EndPoll {
        event_id: String,
    },
    RetrySend {
        local_id: String,
    },
    DiscardSend {
        local_id: String,
    },
    RetryTimeline,
    OpenVideo {
        event_id: String,
    },
    CloseVideo,
    PlayAudio {
        event_id: String,
    },
    CloseAudio,
    ToggleAudio,
    SeekAudio {
        ms: u64,
    },
    WindowFocus {
        focused: bool,
    },
    SwipeTravel {
        px: i32,
    },
    ContextPress,
    OpenLink {
        url: String,
    },
    AcceptVerification,
    RejectVerification,
    ConfirmVerification,
    DismissVerification,
    SessionExpired,
    SoftLogout,
    DismissToast,
    Logout,
    Quit,
}

pub struct Rejected(pub String);

pub enum Driven {
    Command(UiCommand),
    SessionExpiry,
    SoftLogout,
    Poke(Poke),
}

impl fmt::Display for Driven {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(cmd) => write!(f, "{cmd}"),
            Self::SessionExpiry => f.write_str("SessionExpired"),
            Self::SoftLogout => f.write_str("SoftLogout"),
            Self::Poke(Poke::ToggleAudio) => f.write_str("ToggleAudio"),
            Self::Poke(Poke::SeekAudio(position)) => {
                write!(f, "SeekAudio({}ms)", position.as_millis())
            }
            Self::Poke(Poke::WindowFocus(focused)) => write!(f, "WindowFocus({focused})"),
            Self::Poke(Poke::SwipeTravel(px)) => write!(f, "SwipeTravel({px}px)"),
            Self::Poke(Poke::ContextPress) => f.write_str("ContextPress"),
        }
    }
}

pub type Selection<'a> = Option<&'a (String, i32)>;

struct Target {
    room_id: RoomId,
    generation: i32,
}

fn target(
    explicit_room: Option<String>,
    explicit_generation: Option<i32>,
    selected: Selection<'_>,
) -> Result<Target, Rejected> {
    let room_id = explicit_room
        .or_else(|| selected.map(|(id, _)| id.clone()))
        .map(RoomId::new)
        .ok_or_else(|| Rejected("no room_id was given and no room is selected".to_owned()))?;
    let generation = explicit_generation
        .or_else(|| selected.map(|(_, current)| *current))
        .unwrap_or_default();
    Ok(Target {
        room_id,
        generation,
    })
}

fn room(explicit: Option<String>, selected: Selection<'_>) -> Result<RoomId, Rejected> {
    Ok(target(explicit, None, selected)?.room_id)
}

fn room_notify(
    explicit: Option<String>,
    mode: &str,
    selected: Selection<'_>,
) -> Result<UiCommand, Rejected> {
    let mode = names::notify_mode_named(mode).ok_or_else(|| {
        Rejected(format!(
            "{mode} is not a notification mode; use all-messages, mentions-only or muted"
        ))
    })?;
    Ok(UiCommand::SetRoomNotify {
        room_id: room(explicit, selected)?,
        mode,
    })
}

fn message_edit(
    target: Option<EditTarget>,
    caption: bool,
    body: String,
    original: Option<String>,
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
        }),
    }
}

fn poll_draft(
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

#[allow(clippy::too_many_lines)]
pub fn to_driven(command: ProbeCommand, selected: Selection<'_>) -> Result<Driven, Rejected> {
    Ok(Driven::Command(match command {
        ProbeCommand::RetryRestore => UiCommand::RestoreSession,
        ProbeCommand::CheckServer { homeserver } => UiCommand::CheckServer(homeserver),
        ProbeCommand::LoginOauth => UiCommand::LoginOAuth,
        ProbeCommand::CancelOauth => UiCommand::CancelOAuth,
        ProbeCommand::BackToHomeserver => UiCommand::BackToHomeserver,
        ProbeCommand::ReauthOauth => UiCommand::ReauthOAuth,
        ProbeCommand::SelectSpace { space_id } => UiCommand::SelectSpace(space_id.map(RoomId::new)),
        ProbeCommand::SelectDirect => UiCommand::SelectDirect,
        ProbeCommand::SelectSubspace { subspace_id } => {
            UiCommand::SelectSubspace(subspace_id.map(RoomId::new))
        }
        ProbeCommand::MoveSpace { from, to } => UiCommand::MoveSpace { from, to },
        ProbeCommand::SelectRoom { room_id } => UiCommand::SelectRoom(RoomId::new(room_id)),
        ProbeCommand::OpenSpaceIndex => UiCommand::OpenSpaceIndex,
        ProbeCommand::CloseSpaceIndex => UiCommand::CloseSpaceIndex,
        ProbeCommand::PageSpaceIndex => UiCommand::PageSpaceIndex,
        ProbeCommand::RetrySpaceIndex => UiCommand::RetrySpaceIndex,
        ProbeCommand::JoinSpaceChild { room_id } => UiCommand::JoinSpaceChild(RoomId::new(room_id)),
        ProbeCommand::OpenSpaceChild { room_id } => UiCommand::OpenSpaceChild(RoomId::new(room_id)),
        ProbeCommand::OpenRoomInfo { room_id } => UiCommand::OpenRoomInfo(room(room_id, selected)?),
        ProbeCommand::CloseRoomInfo => UiCommand::CloseRoomInfo,
        ProbeCommand::PageRoomMembers => UiCommand::PageRoomMembers,
        ProbeCommand::RetryRoomMembers => UiCommand::RetryRoomMembers,
        ProbeCommand::FilterRoomMembers { query } => UiCommand::FilterRoomMembers(query),
        ProbeCommand::SetRoomNotify { room_id, mode } => room_notify(room_id, &mode, selected)?,
        ProbeCommand::LeaveRoom { room_id } => UiCommand::LeaveRoom(room(room_id, selected)?),
        ProbeCommand::OpenRoomMenu { room_id } => UiCommand::OpenRoomMenu(room(room_id, selected)?),
        ProbeCommand::CloseRoomMenu => UiCommand::CloseRoomMenu,
        ProbeCommand::MarkRoomRead { room_id } => UiCommand::MarkRoomRead(room(room_id, selected)?),
        ProbeCommand::CopyRoomLink { room_id } => UiCommand::CopyRoomLink(room(room_id, selected)?),
        ProbeCommand::SendMessage {
            room_id,
            body,
            reply_to,
            reply_sender,
            reply_preview,
        } => UiCommand::SendMessage {
            room_id: room(room_id, selected)?,
            draft: MessageDraft {
                body,
                reply: reply_to.map(|event_id| ReplyDraft {
                    event_id,
                    sender: reply_sender,
                    preview: reply_preview,
                }),
            },
        },
        ProbeCommand::EditMessage {
            room_id,
            event_id,
            local_id,
            caption,
            body,
            original,
        } => UiCommand::EditMessage {
            room_id: room(room_id, selected)?,
            edit: message_edit(EditTarget::of(event_id, local_id), caption, body, original)?,
        },
        ProbeCommand::DismissUnsent { submission } => UiCommand::DismissUnsent { submission },
        ProbeCommand::SendPoll {
            room_id,
            question,
            answers,
            multiple,
            hide_results,
        } => UiCommand::SendPoll {
            room_id: room(room_id, selected)?,
            draft: poll_draft(question, answers, multiple, hide_results)?,
        },
        ProbeCommand::EditPoll {
            event_id,
            question,
            answers,
            multiple,
            hide_results,
        } => UiCommand::EditPoll {
            event_id,
            draft: poll_draft(question, answers, multiple, hide_results)?,
        },
        ProbeCommand::SendSticker {
            room_id,
            pack,
            shortcode,
            reply_to,
        } => UiCommand::SendSticker {
            room_id: room(room_id, selected)?,
            pack: PackId::new(pack),
            shortcode,
            reply_to,
        },
        ProbeCommand::PickAttachment { room_id, document } => {
            if attachments::scenario().preset_pick.is_none() {
                return Err(Rejected(
                    "PickAttachment opens a native file chooser, which cannot be driven and \
                     blocks the Slint event loop; set U2DM_DEMO_ATTACHMENTS=pick=<path> first"
                        .to_owned(),
                ));
            }
            UiCommand::PickAttachment {
                room_id: room(room_id, selected)?,
                pick: if document {
                    AttachmentPick::Document
                } else {
                    AttachmentPick::Media
                },
            }
        }
        ProbeCommand::SendAttachment {
            room_id,
            caption,
            as_document,
            reply_to,
        } => UiCommand::SendAttachment {
            room_id: room(room_id, selected)?,
            caption,
            as_document,
            reply_to,
        },
        ProbeCommand::CancelAttachment => UiCommand::CancelAttachment,
        ProbeCommand::PaginateBackwards {
            room_id,
            generation,
        } => {
            let at = target(room_id, generation, selected)?;
            UiCommand::PaginateBackwards {
                room_id: at.room_id,
                generation: at.generation,
            }
        }
        ProbeCommand::PaginateForwards {
            room_id,
            generation,
        } => {
            let at = target(room_id, generation, selected)?;
            UiCommand::PaginateForwards {
                room_id: at.room_id,
                generation: at.generation,
            }
        }
        ProbeCommand::JumpToLatest {
            room_id,
            generation,
        } => {
            let at = target(room_id, generation, selected)?;
            UiCommand::JumpToLatest {
                room_id: at.room_id,
                generation: at.generation,
            }
        }
        ProbeCommand::JumpToEvent { event_id } => UiCommand::JumpToEvent { event_id },
        ProbeCommand::OpenPinned { event_id } => UiCommand::OpenPinned { event_id },
        ProbeCommand::CopyMessageLink { event_id } => UiCommand::CopyMessageLink { event_id },
        ProbeCommand::OpenEventSource { event_id } => UiCommand::OpenEventSource { event_id },
        ProbeCommand::CloseEventSource => UiCommand::CloseEventSource,
        ProbeCommand::PinMessage { event_id } => UiCommand::PinMessage { event_id },
        ProbeCommand::UnpinMessage { event_id } => UiCommand::UnpinMessage { event_id },
        ProbeCommand::DeleteMessage { event_id } => UiCommand::DeleteMessage { event_id },
        ProbeCommand::ToggleReaction { event_id, key } => {
            UiCommand::ToggleReaction { event_id, key }
        }
        ProbeCommand::VotePoll {
            event_id,
            answer_id,
        } => UiCommand::VotePoll {
            event_id,
            answer_id,
        },
        ProbeCommand::EndPoll { event_id } => UiCommand::EndPoll { event_id },
        ProbeCommand::RetrySend { local_id } => UiCommand::RetrySend { local_id },
        ProbeCommand::DiscardSend { local_id } => UiCommand::DiscardSend { local_id },
        ProbeCommand::RetryTimeline => UiCommand::RetryTimeline,
        ProbeCommand::OpenVideo { event_id } => {
            if !cfg!(feature = "video") {
                return Err(Rejected(
                    "this build has no video feature, so OpenVideo hands the file to the system \
                     player instead of opening the in-app overlay"
                        .to_owned(),
                ));
            }
            UiCommand::OpenVideo { event_id }
        }
        ProbeCommand::CloseVideo => UiCommand::CloseVideo,
        ProbeCommand::PlayAudio { event_id } => {
            if !cfg!(feature = "video") {
                return Err(Rejected(
                    "this build has no video feature, so PlayAudio hands the file to the system \
                     player instead of playing it in the app"
                        .to_owned(),
                ));
            }
            UiCommand::PlayAudio { event_id }
        }
        ProbeCommand::CloseAudio => UiCommand::CloseAudio,
        ProbeCommand::ToggleAudio => return Ok(Driven::Poke(Poke::ToggleAudio)),
        ProbeCommand::SeekAudio { ms } => {
            return Ok(Driven::Poke(Poke::SeekAudio(Duration::from_millis(ms))));
        }
        ProbeCommand::WindowFocus { focused } => {
            return Ok(Driven::Poke(Poke::WindowFocus(focused)));
        }
        ProbeCommand::SwipeTravel { px } => return Ok(Driven::Poke(Poke::SwipeTravel(px))),
        ProbeCommand::ContextPress => return Ok(Driven::Poke(Poke::ContextPress)),
        ProbeCommand::OpenLink { url } => UiCommand::OpenLink {
            url: LauncherSafeUrl::message_link(&url)
                .ok_or_else(|| Rejected(format!("{url} is not a link U2DM opens")))?,
        },
        ProbeCommand::AcceptVerification => UiCommand::AcceptVerification,
        ProbeCommand::RejectVerification => UiCommand::RejectVerification,
        ProbeCommand::ConfirmVerification => UiCommand::ConfirmVerification,
        ProbeCommand::DismissVerification => UiCommand::DismissVerification,
        ProbeCommand::SessionExpired => return Ok(Driven::SessionExpiry),
        ProbeCommand::SoftLogout => return Ok(Driven::SoftLogout),
        ProbeCommand::DismissToast => UiCommand::DismissToast,
        ProbeCommand::Logout => UiCommand::Logout,
        ProbeCommand::Quit => UiCommand::Quit,
    }))
}
