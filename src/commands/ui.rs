use strum::Display as StrumDisplay;

use crate::domain::auth::LoginCredentials;
use crate::domain::link::LauncherSafeUrl;
use crate::domain::media::AttachmentPick;
use crate::domain::message::MessageEdit;
use crate::domain::poll::PollDraft;
use crate::domain::room::{NotifyMode, RoomId};
use crate::domain::sticker::PackId;
use crate::domain::user_info::UserId;

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
    #[strum(to_string = "OpenRoomInfo({0})")]
    OpenRoomInfo(RoomId),
    CloseRoomInfo,
    ShowRoomInfoPane,
    HideRoomInfoPane,
    PageRoomMembers,
    RetryRoomMembers,
    #[strum(to_string = "FilterRoomMembers")]
    FilterRoomMembers(String),
    #[strum(to_string = "SetRoomNotify({room_id})")]
    SetRoomNotify {
        room_id: RoomId,
        mode: NotifyMode,
    },
    #[strum(to_string = "LeaveRoom({0})")]
    LeaveRoom(RoomId),
    #[strum(to_string = "OpenRoomMenu({0})")]
    OpenRoomMenu(RoomId),
    CloseRoomMenu,
    #[strum(to_string = "MarkRoomRead({0})")]
    MarkRoomRead(RoomId),
    #[strum(to_string = "CopyRoomLink({0})")]
    CopyRoomLink(RoomId),
    #[strum(to_string = "OpenUserInfo({0})")]
    OpenUserInfo(UserId),
    CloseUserInfo,
    RetryUserInfo,
    #[strum(to_string = "MessageUser({0})")]
    MessageUser(UserId),
    #[strum(to_string = "IgnoreUser({0})")]
    IgnoreUser(UserId),
    #[strum(to_string = "UnignoreUser({0})")]
    UnignoreUser(UserId),
    #[strum(to_string = "KickUser({0})")]
    KickUser(UserId),
    #[strum(to_string = "BanUser({0})")]
    BanUser(UserId),
    #[strum(to_string = "UnbanUser({0})")]
    UnbanUser(UserId),
    #[strum(to_string = "SendMessage({room_id})")]
    SendMessage {
        room_id: RoomId,
        draft: MessageDraft,
    },
    #[strum(to_string = "EditMessage({room_id})")]
    EditMessage {
        room_id: RoomId,
        edit: MessageEdit,
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
    #[strum(to_string = "SendPoll({room_id})")]
    SendPoll {
        room_id: RoomId,
        draft: PollDraft,
    },
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
    #[strum(to_string = "CopyMessageLink({event_id})")]
    CopyMessageLink {
        event_id: String,
    },
    #[strum(to_string = "OpenEventSource({event_id})")]
    OpenEventSource {
        event_id: String,
    },
    CloseEventSource,
    #[strum(to_string = "PinMessage({event_id})")]
    PinMessage {
        event_id: String,
    },
    #[strum(to_string = "UnpinMessage({event_id})")]
    UnpinMessage {
        event_id: String,
    },
    #[strum(to_string = "DeleteMessage({event_id})")]
    DeleteMessage {
        event_id: String,
    },
    #[strum(to_string = "ToggleReaction({event_id})")]
    ToggleReaction {
        event_id: String,
        key: String,
    },
    #[strum(to_string = "VotePoll({event_id})")]
    VotePoll {
        event_id: String,
        answer_id: String,
    },
    #[strum(to_string = "EndPoll({event_id})")]
    EndPoll {
        event_id: String,
    },
    #[strum(to_string = "EditPoll({event_id})")]
    EditPoll {
        event_id: String,
        draft: PollDraft,
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
        url: LauncherSafeUrl,
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

#[derive(Clone, PartialEq, Eq)]
pub enum Draft {
    Message(MessageDraft),
    Edit(MessageEdit),
}

impl Draft {
    pub fn body(&self) -> &str {
        match self {
            Self::Message(message) => &message.body,
            Self::Edit(edit) => &edit.body,
        }
    }
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
