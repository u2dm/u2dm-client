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
    #[strum(to_string = "ReauthPassword(...)")]
    ReauthPassword(String),
    ReauthOAuth,
    #[strum(to_string = "SelectSpace")]
    SelectSpace(Option<RoomId>),
    SelectDirect,
    #[strum(to_string = "SelectSubspace")]
    SelectSubspace(Option<RoomId>),
    #[strum(to_string = "MoveSpace({from},{to})")]
    MoveSpace {
        from: usize,
        to: usize,
    },
    #[strum(to_string = "SelectRoom({0})")]
    SelectRoom(RoomId),
    OpenSpaceIndex,
    CloseSpaceIndex,
    PageSpaceIndex,
    RetrySpaceIndex,
    #[strum(to_string = "JoinSpaceChild({0})")]
    JoinSpaceChild(RoomId),
    #[strum(to_string = "OpenSpaceChild({0})")]
    OpenSpaceChild(RoomId),
    #[strum(to_string = "SendMessage({room_id})")]
    SendMessage {
        room_id: RoomId,
        draft: MessageDraft,
    },
    #[strum(to_string = "DismissUnsent({submission})")]
    DismissUnsent {
        submission: i32,
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
    #[strum(to_string = "OpenPinned({event_id})")]
    OpenPinned {
        event_id: String,
    },
    #[strum(to_string = "ToggleReaction({event_id})")]
    ToggleReaction {
        event_id: String,
        key: String,
    },
    #[strum(to_string = "RetrySend({local_id})")]
    RetrySend { local_id: String },
    #[strum(to_string = "DiscardSend({local_id})")]
    DiscardSend { local_id: String },
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
    #[strum(to_string = "PlayAudio({event_id})")]
    PlayAudio {
        event_id: String,
    },
    CloseAudio,
    #[strum(to_string = "AudioEnded({request}, {end})")]
    AudioEnded {
        request: u64,
        end: AudioEnd,
    },
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

#[derive(Clone, PartialEq, Eq)]
pub struct MessageDraft {
    pub body: String,
    pub reply: Option<ReplyDraft>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ReplyDraft {
    pub event_id: String,
    pub sender: String,
    pub preview: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, StrumDisplay)]
pub enum AudioEnd {
    Finished,
    Failed,
}

#[derive(Clone)]
pub struct ViewportChanged {
    pub room_id: RoomId,
    pub generation: i32,
    pub at_bottom: bool,
    pub unread_below: u32,
}

impl ViewportChanged {
    pub fn initial() -> Self {
        Self {
            room_id: RoomId::new(String::new()),
            generation: 0,
            at_bottom: true,
            unread_below: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TimelineVisibility {
    Visible,
    #[default]
    Hidden,
}
