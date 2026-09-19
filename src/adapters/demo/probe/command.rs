use std::fmt;

use serde::Deserialize;

use std::time::Duration;

use crate::adapters::demo::attachments;
use crate::adapters::ui::dump::Poke;
use crate::commands::ui::{MessageDraft, ReplyDraft, UiCommand};
use crate::domain::media::AttachmentPick;
use crate::domain::room::RoomId;
use crate::domain::sticker::PackId;

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProbeCommand {
    CheckServer {
        homeserver: String,
    },
    LoginOauth,
    CancelOauth,
    BackToHomeserver,
    SelectSpace {
        #[serde(default)]
        space_id: Option<String>,
    },
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
    DismissUnsent {
        submission: i32,
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
    ToggleReaction {
        event_id: String,
        key: String,
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
    OpenLink {
        url: String,
    },
    AcceptVerification,
    RejectVerification,
    ConfirmVerification,
    DismissVerification,
    SessionExpired,
    DismissToast,
    Logout,
    Quit,
}

pub struct Rejected(pub String);

pub enum Driven {
    Command(UiCommand),
    SessionExpiry,
    Poke(Poke),
}

impl fmt::Display for Driven {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(cmd) => write!(f, "{cmd}"),
            Self::SessionExpiry => f.write_str("SessionExpired"),
            Self::Poke(Poke::ToggleAudio) => f.write_str("ToggleAudio"),
            Self::Poke(Poke::SeekAudio(position)) => {
                write!(f, "SeekAudio({}ms)", position.as_millis())
            }
            Self::Poke(Poke::WindowFocus(focused)) => write!(f, "WindowFocus({focused})"),
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

#[allow(clippy::too_many_lines)]
pub fn to_driven(command: ProbeCommand, selected: Selection<'_>) -> Result<Driven, Rejected> {
    Ok(Driven::Command(match command {
        ProbeCommand::CheckServer { homeserver } => UiCommand::CheckServer(homeserver),
        ProbeCommand::LoginOauth => UiCommand::LoginOAuth,
        ProbeCommand::CancelOauth => UiCommand::CancelOAuth,
        ProbeCommand::BackToHomeserver => UiCommand::BackToHomeserver,
        ProbeCommand::SelectSpace { space_id } => UiCommand::SelectSpace(space_id.map(RoomId::new)),
        ProbeCommand::SelectSubspace { subspace_id } => {
            UiCommand::SelectSubspace(subspace_id.map(RoomId::new))
        }
        ProbeCommand::MoveSpace { from, to } => UiCommand::MoveSpace { from, to },
        ProbeCommand::SelectRoom { room_id } => UiCommand::SelectRoom(RoomId::new(room_id)),
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
        ProbeCommand::DismissUnsent { submission } => UiCommand::DismissUnsent { submission },
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
        ProbeCommand::ToggleReaction { event_id, key } => {
            UiCommand::ToggleReaction { event_id, key }
        }
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
        ProbeCommand::OpenLink { url } => UiCommand::OpenLink { url },
        ProbeCommand::AcceptVerification => UiCommand::AcceptVerification,
        ProbeCommand::RejectVerification => UiCommand::RejectVerification,
        ProbeCommand::ConfirmVerification => UiCommand::ConfirmVerification,
        ProbeCommand::DismissVerification => UiCommand::DismissVerification,
        ProbeCommand::SessionExpired => return Ok(Driven::SessionExpiry),
        ProbeCommand::DismissToast => UiCommand::DismissToast,
        ProbeCommand::Logout => UiCommand::Logout,
        ProbeCommand::Quit => UiCommand::Quit,
    }))
}
