use strum::Display as StrumDisplay;

use crate::domain::auth::LoginCredentials;
use crate::domain::call::devices::{DeviceKey, DeviceKind};
use crate::domain::call::media::MediaChoice;
use crate::domain::capabilities::AccountPage;
use crate::domain::emote::{ComposerText, Emote};
use crate::domain::encryption::{KeyPassphrase, RecoveryKey};
use crate::domain::image_pack::PackId;
use crate::domain::link::LauncherSafeUrl;
use crate::domain::matrix_link::{AddressEntry, MatrixLink, RoomAddress, RoomLink};
use crate::domain::media::AttachmentPick;
use crate::domain::message::MessageEdit;
use crate::domain::poll::PollDraft;
use crate::domain::room::{NotifyMode, RoomId};
use crate::domain::room_creation::RoomDraft;
use crate::domain::uia::AccountPassword;
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
    #[strum(to_string = "OpenNotification({0})")]
    OpenNotification(RoomId),
    #[strum(to_string = "FilterRooms")]
    FilterRooms(String),
    FoldInvites,
    UnfoldInvites,
    #[strum(to_string = "OpenSpace({0})")]
    OpenSpace(RoomId),
    OpenSpaceIndex,
    CloseSpaceIndex,
    PageSpaceIndex,
    RetrySpaceIndex,
    #[strum(to_string = "JoinSpaceChild({0})")]
    JoinSpaceChild(RoomId),
    #[strum(to_string = "OpenSpaceChild({0})")]
    OpenSpaceChild(RoomId),
    #[strum(to_string = "OpenInvite({0})")]
    OpenInvite(RoomId),
    CloseInvite,
    #[strum(to_string = "AcceptInvite({0})")]
    AcceptInvite(RoomId),
    #[strum(to_string = "DeclineInvite({0})")]
    DeclineInvite(RoomId),
    #[strum(to_string = "OpenRemoval({0})")]
    OpenRemoval(RoomId),
    CloseRemoval,
    #[strum(to_string = "ForgetRoom({0})")]
    ForgetRoom(RoomId),
    #[strum(to_string = "JoinSuccessor({0})")]
    JoinSuccessor(RoomId),
    #[strum(to_string = "JoinPredecessor({0})")]
    JoinPredecessor(RoomId),
    #[strum(to_string = "OpenRoomLink")]
    OpenRoomLink(RoomLink),
    #[strum(to_string = "OpenExternalLink")]
    OpenExternalLink(MatrixLink),
    OpenRoomFinder,
    #[strum(to_string = "FindRoom")]
    FindRoom(AddressEntry),
    #[strum(to_string = "JoinPreviewedRoom({0})")]
    JoinPreviewedRoom(String),
    RetryRoomPreview,
    CloseRoomPreview,
    OpenCreateRoom,
    OpenCreateSpace,
    CloseCreateRoom,
    #[strum(to_string = "CreateRoom")]
    CreateRoom(RoomDraft),
    #[strum(to_string = "OpenInvitePeople({0})")]
    OpenInvitePeople(RoomId),
    CloseInvitePeople,
    #[strum(to_string = "SearchPeople")]
    SearchPeople(String),
    RetryPeopleSearch,
    #[strum(to_string = "PickInvitee({0})")]
    PickInvitee(UserId),
    #[strum(to_string = "UnpickInvitee({0})")]
    UnpickInvitee(UserId),
    #[strum(to_string = "SendInvites")]
    SendInvites(String),
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
    #[strum(to_string = "OpenLeaveSpace({0})")]
    OpenLeaveSpace(RoomId),
    CloseLeaveSpace,
    #[strum(to_string = "LeaveSpace({space_id}, {with_rooms})")]
    LeaveSpace {
        space_id: RoomId,
        with_rooms: bool,
    },
    #[strum(to_string = "OpenRoomMenu({0})")]
    OpenRoomMenu(RoomId),
    CloseRoomMenu,
    #[strum(to_string = "MarkRoomRead({0})")]
    MarkRoomRead(RoomId),
    #[strum(to_string = "CopyRoomLink({0})")]
    CopyRoomLink(RoomId),
    #[strum(to_string = "OpenRoomLog({0})")]
    OpenRoomLog(RoomId),
    CloseRoomLog,
    #[strum(to_string = "OpenUserInfo({0})")]
    OpenUserInfo(UserId),
    CloseUserInfo,
    RetryUserInfo,
    #[strum(to_string = "MessageUser({0})")]
    MessageUser(UserId),
    #[strum(to_string = "VerifyUser({0})")]
    VerifyUser(UserId),
    #[strum(to_string = "IgnoreUser({0})")]
    IgnoreUser(UserId),
    #[strum(to_string = "UnignoreUser({0})")]
    UnignoreUser(UserId),
    #[strum(to_string = "KickUser({user_id})")]
    KickUser {
        user_id: UserId,
        reason: Option<String>,
    },
    #[strum(to_string = "BanUser({user_id})")]
    BanUser {
        user_id: UserId,
        reason: Option<String>,
    },
    #[strum(to_string = "UnbanUser({0})")]
    UnbanUser(UserId),
    #[strum(to_string = "AcceptIdentity({0})")]
    AcceptIdentity(UserId),
    #[strum(to_string = "WithdrawVerification({0})")]
    WithdrawVerification(UserId),
    AcknowledgeIdentityReset,
    OpenEncryption,
    CloseEncryption,
    ShowEncryptionOverview,
    EnterRecoveryKey,
    #[strum(to_string = "UseRecoveryKey(...)")]
    UseRecoveryKey(RecoveryKey),
    VerifyWithOtherSession,
    SetUpRecovery,
    ChangeRecoveryKey,
    RetryEncryptionSetup,
    StartIdentityReset,
    ResetIdentity,
    StartKeyExport,
    #[strum(to_string = "ExportRoomKeys(...)")]
    ExportRoomKeys(KeyPassphrase),
    StartKeyImport,
    #[strum(to_string = "ImportRoomKeys(...)")]
    ImportRoomKeys(KeyPassphrase),
    OpenAccount,
    CloseAccount,
    ShowAccountOverview,
    #[strum(to_string = "RenameSession({device_id})")]
    RenameSession {
        device_id: String,
        name: String,
    },
    #[strum(to_string = "SignOutSession({0})")]
    SignOutSession(String),
    #[strum(to_string = "OpenAccountPage")]
    OpenAccountPage(AccountPage),
    StartPasswordChange,
    #[strum(to_string = "ChangePassword(...)")]
    ChangePassword {
        password: AccountPassword,
        sign_out_others: bool,
    },
    StartDeactivation,
    #[strum(to_string = "DeactivateAccount({erase})")]
    DeactivateAccount {
        erase: bool,
    },
    #[strum(to_string = "AnswerAuthPassword(...)")]
    AnswerAuthPassword(AccountPassword),
    ContinueAuth,
    ReopenAuthPage,
    CancelAuth,
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
    #[strum(to_string = "SuggestMentions")]
    SuggestMentions {
        room_id: RoomId,
        query: String,
        recent: Vec<UserId>,
    },
    EndMentions,
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
        caption: ComposerText,
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
    #[strum(to_string = "PreviousPinned({event_id})")]
    PreviousPinned {
        event_id: String,
    },
    #[strum(to_string = "NextPinned({event_id})")]
    NextPinned {
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
    #[strum(to_string = "OpenReaders({event_id})")]
    OpenReaders {
        event_id: String,
    },
    CloseReaders,
    PageReaders,
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
    #[strum(to_string = "ReactWithEmote({event_id})")]
    ReactWithEmote {
        event_id: String,
        pack: PackId,
        shortcode: String,
    },
    #[strum(to_string = "SetPackGlobal({pack}, {global})")]
    SetPackGlobal {
        pack: PackId,
        global: bool,
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
    #[strum(to_string = "DeclineCall({0})")]
    DeclineCall(RoomId),
    SilenceRing,
    #[strum(to_string = "OpenCallSheet({room_id})")]
    OpenCallSheet {
        room_id: RoomId,
        video: bool,
    },
    CloseCallSheet,
    #[strum(to_string = "SheetCamera({0})")]
    SheetCamera(bool),
    #[strum(to_string = "TestCallDevice({kind}, {on})")]
    TestCallDevice {
        kind: DeviceKind,
        on: bool,
    },
    #[strum(to_string = "StartCall({room_id})")]
    StartCall {
        room_id: RoomId,
        video: bool,
        media: MediaChoice,
        leaving: Option<RoomId>,
    },
    #[strum(to_string = "JoinCall({room_id})")]
    JoinCall {
        room_id: RoomId,
        media: MediaChoice,
        leaving: Option<RoomId>,
    },
    #[strum(to_string = "AnswerCall({room_id})")]
    AnswerCall {
        room_id: RoomId,
        camera: bool,
        leaving: Option<RoomId>,
    },
    LeaveCall,
    RetryFailedCall,
    DismissFailedCall,
    AnnounceCallAgain,
    #[strum(to_string = "SetMicrophone({0})")]
    SetMicrophone(bool),
    #[strum(to_string = "SetCamera({0})")]
    SetCamera(bool),
    #[strum(to_string = "ShareScreen({0})")]
    ShareScreen(bool),
    ChangeScreenShare,
    #[strum(to_string = "UseCallDevice({kind})")]
    UseCallDevice {
        kind: DeviceKind,
        key: Option<DeviceKey>,
    },
    RefreshCallDevices,
    #[strum(to_string = "FocusCallTile({0})")]
    FocusCallTile(String),
    UnpinCallTile,
    #[strum(to_string = "RingDevices({0})")]
    RingDevices(bool),
    ShowCallGallery,
    ViewCallShare,
    #[strum(to_string = "ShowSelfView({0})")]
    ShowSelfView(bool),
    #[strum(to_string = "ShowSharePreview({0})")]
    ShowSharePreview(bool),
    #[strum(to_string = "FollowSpeaker({0})")]
    FollowSpeaker(bool),
    #[strum(to_string = "WatchMicrophone({0})")]
    WatchMicrophone(bool),
    #[strum(to_string = "WatchConnections({0})")]
    WatchConnections(bool),
    #[strum(to_string = "MuteForMe({on})")]
    MuteForMe {
        user_id: String,
        on: bool,
    },
    DismissToast,
    Logout,
    ConfirmLogout,
    CancelLogout,
    Quit,
}

impl UiCommand {
    #[allow(clippy::too_many_lines)]
    pub fn room_id(&self) -> Option<&RoomId> {
        match self {
            Self::SelectRoom(room_id)
            | Self::OpenNotification(room_id)
            | Self::OpenSpace(room_id)
            | Self::JoinSpaceChild(room_id)
            | Self::OpenSpaceChild(room_id)
            | Self::OpenInvite(room_id)
            | Self::AcceptInvite(room_id)
            | Self::DeclineInvite(room_id)
            | Self::OpenRemoval(room_id)
            | Self::ForgetRoom(room_id)
            | Self::JoinSuccessor(room_id)
            | Self::JoinPredecessor(room_id)
            | Self::OpenRoomLink(RoomLink {
                room: RoomAddress::Id(room_id),
                ..
            })
            | Self::OpenRoomInfo(room_id)
            | Self::OpenInvitePeople(room_id)
            | Self::LeaveRoom(room_id)
            | Self::OpenLeaveSpace(room_id)
            | Self::LeaveSpace {
                space_id: room_id, ..
            }
            | Self::OpenRoomMenu(room_id)
            | Self::MarkRoomRead(room_id)
            | Self::CopyRoomLink(room_id)
            | Self::OpenRoomLog(room_id)
            | Self::SetRoomNotify { room_id, .. }
            | Self::SendMessage { room_id, .. }
            | Self::EditMessage { room_id, .. }
            | Self::PickAttachment { room_id, .. }
            | Self::SendAttachment { room_id, .. }
            | Self::SendPoll { room_id, .. }
            | Self::SendSticker { room_id, .. }
            | Self::PaginateBackwards { room_id, .. }
            | Self::PaginateForwards { room_id, .. }
            | Self::JumpToLatest { room_id, .. }
            | Self::DeclineCall(room_id)
            | Self::OpenCallSheet { room_id, .. }
            | Self::StartCall { room_id, .. }
            | Self::JoinCall { room_id, .. }
            | Self::AnswerCall { room_id, .. } => Some(room_id),
            Self::RestoreSession
            | Self::CheckServer(_)
            | Self::LoginPassword(_)
            | Self::LoginOAuth
            | Self::CancelOAuth
            | Self::BackToHomeserver
            | Self::ReauthPassword(_)
            | Self::ReauthOAuth
            | Self::SelectSpace(_)
            | Self::SelectDirect
            | Self::SelectSubspace(_)
            | Self::MoveSpace { .. }
            | Self::FilterRooms(_)
            | Self::FoldInvites
            | Self::UnfoldInvites
            | Self::OpenSpaceIndex
            | Self::CloseSpaceIndex
            | Self::PageSpaceIndex
            | Self::RetrySpaceIndex
            | Self::CloseInvite
            | Self::CloseRemoval
            | Self::CloseLeaveSpace
            | Self::OpenRoomLink(_)
            | Self::OpenExternalLink(_)
            | Self::OpenRoomFinder
            | Self::FindRoom(_)
            | Self::JoinPreviewedRoom(_)
            | Self::RetryRoomPreview
            | Self::CloseRoomPreview
            | Self::OpenCreateRoom
            | Self::OpenCreateSpace
            | Self::CloseCreateRoom
            | Self::CreateRoom(_)
            | Self::CloseInvitePeople
            | Self::SearchPeople(_)
            | Self::RetryPeopleSearch
            | Self::PickInvitee(_)
            | Self::UnpickInvitee(_)
            | Self::SendInvites(_)
            | Self::CloseRoomInfo
            | Self::ShowRoomInfoPane
            | Self::HideRoomInfoPane
            | Self::PageRoomMembers
            | Self::RetryRoomMembers
            | Self::FilterRoomMembers(_)
            | Self::CloseRoomMenu
            | Self::CloseRoomLog
            | Self::OpenUserInfo(_)
            | Self::CloseUserInfo
            | Self::RetryUserInfo
            | Self::MessageUser(_)
            | Self::VerifyUser(_)
            | Self::IgnoreUser(_)
            | Self::UnignoreUser(_)
            | Self::KickUser { .. }
            | Self::BanUser { .. }
            | Self::UnbanUser(_)
            | Self::AcceptIdentity(_)
            | Self::WithdrawVerification(_)
            | Self::AcknowledgeIdentityReset
            | Self::OpenEncryption
            | Self::CloseEncryption
            | Self::ShowEncryptionOverview
            | Self::EnterRecoveryKey
            | Self::UseRecoveryKey(_)
            | Self::VerifyWithOtherSession
            | Self::SetUpRecovery
            | Self::ChangeRecoveryKey
            | Self::RetryEncryptionSetup
            | Self::StartIdentityReset
            | Self::ResetIdentity
            | Self::StartKeyExport
            | Self::ExportRoomKeys(_)
            | Self::StartKeyImport
            | Self::ImportRoomKeys(_)
            | Self::OpenAccount
            | Self::CloseAccount
            | Self::ShowAccountOverview
            | Self::RenameSession { .. }
            | Self::SignOutSession(_)
            | Self::OpenAccountPage(_)
            | Self::StartPasswordChange
            | Self::ChangePassword { .. }
            | Self::StartDeactivation
            | Self::DeactivateAccount { .. }
            | Self::AnswerAuthPassword(_)
            | Self::ContinueAuth
            | Self::ReopenAuthPage
            | Self::CancelAuth
            | Self::SuggestMentions { .. }
            | Self::EndMentions
            | Self::DismissUnsent { .. }
            | Self::CancelAttachment
            | Self::JumpToEvent { .. }
            | Self::OpenPinned { .. }
            | Self::PreviousPinned { .. }
            | Self::NextPinned { .. }
            | Self::CopyMessageLink { .. }
            | Self::OpenEventSource { .. }
            | Self::CloseEventSource
            | Self::OpenReaders { .. }
            | Self::CloseReaders
            | Self::PageReaders
            | Self::PinMessage { .. }
            | Self::UnpinMessage { .. }
            | Self::DeleteMessage { .. }
            | Self::ToggleReaction { .. }
            | Self::ReactWithEmote { .. }
            | Self::SetPackGlobal { .. }
            | Self::VotePoll { .. }
            | Self::EndPoll { .. }
            | Self::EditPoll { .. }
            | Self::RetrySend { .. }
            | Self::DiscardSend { .. }
            | Self::RetryTimeline
            | Self::AcceptVerification
            | Self::RejectVerification
            | Self::ConfirmVerification
            | Self::DismissVerification
            | Self::OpenMedia { .. }
            | Self::OpenVideo { .. }
            | Self::CloseVideo
            | Self::PlayAudio { .. }
            | Self::CloseAudio
            | Self::AudioEnded { .. }
            | Self::OpenLink { .. }
            | Self::SaveFile { .. }
            | Self::SilenceRing
            | Self::CloseCallSheet
            | Self::SheetCamera(_)
            | Self::TestCallDevice { .. }
            | Self::LeaveCall
            | Self::RetryFailedCall
            | Self::DismissFailedCall
            | Self::AnnounceCallAgain
            | Self::SetMicrophone(_)
            | Self::SetCamera(_)
            | Self::ShareScreen(_)
            | Self::ChangeScreenShare
            | Self::UseCallDevice { .. }
            | Self::RefreshCallDevices
            | Self::FocusCallTile(_)
            | Self::UnpinCallTile
            | Self::RingDevices(_)
            | Self::ShowCallGallery
            | Self::ViewCallShare
            | Self::ShowSelfView(_)
            | Self::ShowSharePreview(_)
            | Self::FollowSpeaker(_)
            | Self::MuteForMe { .. }
            | Self::WatchMicrophone(_)
            | Self::WatchConnections(_)
            | Self::DismissToast
            | Self::Logout
            | Self::ConfirmLogout
            | Self::CancelLogout
            | Self::Quit => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct MessageDraft {
    pub body: String,
    pub reply: Option<ReplyDraft>,
    pub emotes: Vec<Emote>,
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

    pub fn emotes(&self) -> &[Emote] {
        match self {
            Self::Message(message) => &message.emotes,
            Self::Edit(edit) => &edit.emotes,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WindowFocus {
    #[default]
    Focused,
    Unfocused,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UserActivity {
    #[default]
    Active,
    Idle,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowAttention {
    pub focus: WindowFocus,
    pub activity: UserActivity,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposerActivity {
    pub room_id: Option<RoomId>,
    pub composing: bool,
    pub edits: i32,
}
