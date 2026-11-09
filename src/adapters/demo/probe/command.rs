mod drafts;
mod driven;
mod targets;

use serde::Deserialize;

use self::drafts::{
    EmoteDto, default_room_access, default_room_kind, encrypted_by_default, in_space_by_default,
};
pub(super) use self::driven::{Driven, to_driven};
use self::targets::device_kind;
use crate::domain::call::devices::DeviceKind;

fn signs_out_others() -> bool {
    true
}

fn microphone_on() -> bool {
    true
}

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
    FilterRooms {
        query: String,
    },
    FoldInvites,
    UnfoldInvites,
    OpenSpace {
        space_id: String,
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
    OpenInvite {
        room_id: String,
    },
    CloseInvite,
    AcceptInvite {
        room_id: String,
    },
    DeclineInvite {
        room_id: String,
    },
    OpenRemoval {
        room_id: String,
    },
    CloseRemoval,
    ForgetRoom {
        room_id: String,
    },
    OpenLeaveSpace {
        space_id: String,
    },
    CloseLeaveSpace,
    LeaveSpace {
        space_id: String,
        #[serde(default)]
        with_rooms: bool,
    },
    DeclineCall {
        room_id: String,
    },
    SilenceRing,
    OpenCallSheet {
        #[serde(default)]
        room_id: Option<String>,
        #[serde(default)]
        video: bool,
    },
    CloseCallSheet,
    StartCall {
        #[serde(default)]
        room_id: Option<String>,
        #[serde(default)]
        video: bool,
        #[serde(default = "microphone_on")]
        microphone: bool,
        #[serde(default)]
        camera: bool,
        #[serde(default)]
        leaving: Option<String>,
    },
    JoinCall {
        #[serde(default)]
        room_id: Option<String>,
        #[serde(default = "microphone_on")]
        microphone: bool,
        #[serde(default)]
        camera: bool,
        #[serde(default)]
        leaving: Option<String>,
    },
    AnswerCall {
        room_id: String,
        #[serde(default)]
        camera: bool,
        #[serde(default)]
        leaving: Option<String>,
    },
    LeaveCall,
    RetryFailedCall,
    DismissFailedCall,
    AnnounceCallAgain,
    SetMicrophone {
        on: bool,
    },
    SetCamera {
        on: bool,
    },
    ShareScreen {
        on: bool,
    },
    ChangeScreenShare,
    UseCallDevice {
        #[serde(deserialize_with = "device_kind")]
        kind: DeviceKind,
        #[serde(default)]
        key: Option<String>,
    },
    RefreshCallDevices,
    FocusCallTile {
        key: String,
    },
    UnpinCallTile,
    RingDevices {
        open: bool,
    },
    MuteForMe {
        user_id: String,
        on: bool,
    },
    RaiseHand {
        on: bool,
    },
    SendCallReaction {
        name: String,
    },
    WatchMicrophone {
        on: bool,
    },
    WatchConnections {
        on: bool,
    },
    ShowCallGallery,
    ViewCallShare,
    ShowSelfView {
        on: bool,
    },
    ShowSharePreview {
        on: bool,
    },
    FollowSpeaker {
        on: bool,
    },
    JoinSuccessor {
        #[serde(default)]
        room_id: Option<String>,
    },
    OpenRoomLink {
        #[serde(default)]
        room_id: Option<String>,
        #[serde(default)]
        alias: Option<String>,
        #[serde(default)]
        event_id: Option<String>,
        #[serde(default)]
        via: Vec<String>,
    },
    OpenRoomFinder,
    FindRoom {
        address: String,
    },
    JoinPreviewedRoom {
        address: String,
    },
    RetryRoomPreview,
    CloseRoomPreview,
    OpenCreateRoom,
    OpenCreateSpace,
    CloseCreateRoom,
    CreateRoom {
        name: String,
        #[serde(default)]
        topic: String,
        #[serde(default = "default_room_kind")]
        kind: String,
        #[serde(default = "default_room_access")]
        access: String,
        #[serde(default)]
        address: String,
        #[serde(default = "encrypted_by_default")]
        encrypted: bool,
        #[serde(default = "in_space_by_default")]
        in_space: bool,
    },
    OpenInvitePeople {
        #[serde(default)]
        room_id: Option<String>,
    },
    CloseInvitePeople,
    SearchPeople {
        query: String,
    },
    RetryPeopleSearch,
    PickInvitee {
        user_id: String,
    },
    UnpickInvitee {
        user_id: String,
    },
    SendInvites {
        #[serde(default)]
        reason: String,
    },
    JoinPredecessor {
        #[serde(default)]
        room_id: Option<String>,
    },
    OpenRoomInfo {
        #[serde(default)]
        room_id: Option<String>,
    },
    CloseRoomInfo,
    ShowRoomInfoPane,
    HideRoomInfoPane,
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
    OpenRoomLog {
        #[serde(default)]
        room_id: Option<String>,
    },
    CloseRoomLog,
    OpenUserInfo {
        user_id: String,
    },
    CloseUserInfo,
    RetryUserInfo,
    MessageUser {
        user_id: String,
    },
    VerifyUser {
        user_id: String,
    },
    IgnoreUser {
        user_id: String,
    },
    UnignoreUser {
        user_id: String,
    },
    KickUser {
        user_id: String,
        #[serde(default)]
        reason: String,
    },
    BanUser {
        user_id: String,
        #[serde(default)]
        reason: String,
    },
    UnbanUser {
        user_id: String,
    },
    AcceptIdentity {
        user_id: String,
    },
    WithdrawVerification {
        user_id: String,
    },
    AcknowledgeIdentityReset,
    OpenEncryption,
    CloseEncryption,
    ShowEncryptionOverview,
    EnterRecoveryKey,
    UseRecoveryKey {
        key: String,
    },
    VerifyWithOtherSession,
    SetUpRecovery,
    ChangeRecoveryKey,
    RetryEncryptionSetup,
    StartIdentityReset,
    ResetIdentity,
    StartKeyExport,
    ExportRoomKeys {
        passphrase: String,
    },
    StartKeyImport,
    ImportRoomKeys {
        passphrase: String,
    },
    OpenSettings,
    CloseSettings,
    ShowSettingsPage {
        page: String,
    },
    SetSetting {
        name: String,
        value: String,
    },
    OpenAccount,
    CloseAccount,
    ShowAccountOverview,
    RenameSession {
        device_id: String,
        name: String,
    },
    SignOutSession {
        device_id: String,
    },
    OpenAccountPage,
    OpenSessionsPage,
    StartPasswordChange,
    ChangePassword {
        password: String,
        #[serde(default = "signs_out_others")]
        sign_out_others: bool,
    },
    StartDeactivation,
    DeactivateAccount {
        #[serde(default)]
        erase: bool,
    },
    AnswerAuthPassword {
        password: String,
    },
    ContinueAuth,
    ReopenAuthPage,
    CancelAuth,
    ConfirmLogout,
    CancelLogout,
    SendMessage {
        #[serde(default)]
        room_id: Option<String>,
        body: String,
        #[serde(default)]
        emotes: Vec<EmoteDto>,
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
        #[serde(default)]
        emotes: Vec<EmoteDto>,
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
        caption_emotes: Vec<EmoteDto>,
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
    PreviousPinned {
        event_id: String,
    },
    NextPinned {
        event_id: String,
    },
    CopyMessageLink {
        event_id: String,
    },
    OpenEventSource {
        event_id: String,
    },
    CloseEventSource,
    OpenReaders {
        event_id: String,
    },
    CloseReaders,
    PageReaders,
    SuggestMentions {
        #[serde(default)]
        room_id: Option<String>,
        query: String,
        #[serde(default)]
        recent: Vec<String>,
    },
    EndMentions,
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
    ReactWithEmote {
        event_id: String,
        pack: String,
        shortcode: String,
    },
    SetPackGlobal {
        pack: String,
        global: bool,
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
    SaveFile {
        event_id: String,
        filename: String,
        #[serde(default)]
        mimetype: Option<String>,
    },
    ToggleAudio,
    SeekAudio {
        ms: u64,
    },
    WindowFocus {
        focused: bool,
    },
    ResizeWindow {
        width: f32,
        height: f32,
    },
    UserActivity {
        active: bool,
    },
    SwipeTravel {
        px: i32,
    },
    TouchpadPinch {
        permille: i32,
    },
    ContextPress,
    CloseCallWindow,
    ClickNotification {
        room_id: String,
        #[serde(default)]
        token: Option<String>,
        #[serde(default)]
        action: Option<String>,
    },
    OpenLink {
        url: String,
    },
    OpenExternalLink {
        url: String,
    },
    HandOff {
        #[serde(default)]
        link: Option<String>,
        #[serde(default)]
        token: Option<String>,
    },
    AcceptVerification,
    RejectVerification,
    ConfirmVerification,
    DismissVerification,
    SessionExpired,
    SoftLogout,
    SessionRefused,
    DismissToast,
    Logout,
    Quit,
}

pub struct Rejected(pub String);
