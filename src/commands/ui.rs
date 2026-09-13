use strum::Display as StrumDisplay;

use crate::domain::auth::LoginCredentials;
use crate::domain::media::AttachmentPick;
use crate::domain::room::RoomId;
use crate::domain::sticker::PackId;

#[derive(StrumDisplay)]
pub enum UiCommand {
    RestoreSession,
    #[strum(to_string = "CheckServer({0})")]
    CheckServer(String),
    #[strum(to_string = "LoginPassword(...)")]
    LoginPassword(LoginCredentials),
    LoginOAuth,
    CancelOAuth,
    BackToHomeserver,
    #[strum(to_string = "SelectSpace")]
    SelectSpace(Option<RoomId>),
    #[strum(to_string = "SelectSubspace")]
    SelectSubspace(Option<RoomId>),
    #[strum(to_string = "MoveSpace({from},{to})")]
    MoveSpace {
        from: usize,
        to: usize,
    },
    #[strum(to_string = "SelectRoom({0})")]
    SelectRoom(RoomId),
    #[strum(to_string = "SendMessage({room_id})")]
    SendMessage {
        room_id: RoomId,
        body: String,
        reply_to: Option<String>,
    },
    #[strum(to_string = "PickAttachment({room_id})")]
    PickAttachment {
        room_id: RoomId,
        pick: AttachmentPick,
    },
    #[strum(to_string = "SendAttachment({room_id})")]
    SendAttachment {
        room_id: RoomId,
        caption: String,
        as_document: bool,
        reply_to: Option<String>,
    },
    CancelAttachment,
    #[strum(to_string = "SendSticker({room_id},{shortcode})")]
    SendSticker {
        room_id: RoomId,
        pack: PackId,
        shortcode: String,
        reply_to: Option<String>,
    },
    #[strum(to_string = "PaginateBackwards({room_id})")]
    PaginateBackwards {
        room_id: RoomId,
        generation: i32,
    },
    #[strum(to_string = "PaginateForwards({room_id})")]
    PaginateForwards {
        room_id: RoomId,
        generation: i32,
    },
    #[strum(to_string = "JumpToLatest({room_id})")]
    JumpToLatest {
        room_id: RoomId,
        generation: i32,
    },
    #[strum(to_string = "JumpToEvent({event_id})")]
    JumpToEvent {
        event_id: String,
    },
    #[strum(to_string = "ToggleReaction({event_id})")]
    ToggleReaction {
        event_id: String,
        key: String,
    },
    RetryTimeline,
    AcceptVerification,
    RejectVerification,
    ConfirmVerification,
    DismissVerification,
    #[strum(to_string = "OpenMedia({event_id})")]
    OpenMedia {
        event_id: String,
    },
    #[strum(to_string = "OpenVideo({event_id})")]
    OpenVideo {
        event_id: String,
    },
    #[strum(to_string = "CloseVideo")]
    CloseVideo,
    #[strum(to_string = "OpenLink")]
    OpenLink {
        url: String,
    },
    #[strum(to_string = "SaveFile({filename})")]
    SaveFile {
        event_id: String,
        filename: String,
    },
    DismissToast,
    Logout,
    Quit,
}

#[derive(Clone)]
pub struct ViewportChanged {
    pub room_id: RoomId,
    pub generation: i32,
    pub at_bottom: bool,
}

impl ViewportChanged {
    pub fn initial() -> Self {
        Self {
            room_id: RoomId::new(String::new()),
            generation: 0,
            at_bottom: true,
        }
    }
}
