use std::fmt;

use serde::{Deserialize, Deserializer, de};

use std::time::Duration;

use super::names;
use crate::adapters::demo::attachments;
use crate::adapters::ui::dump::Poke;
use crate::adapters::ui::{link_command, matrix_link, room_address_entry};
use crate::commands::ui::{MessageDraft, ReplyDraft, UiCommand};
use crate::domain::call::devices::{DeviceKey, DeviceKind};
use crate::domain::call::media::MediaChoice;
use crate::domain::capabilities::AccountPage;
use crate::domain::departure::membership_reason;
use crate::domain::emote::{ComposerText, Emote};
use crate::domain::encryption::{KeyPassphrase, RecoveryKey};
use crate::domain::image_pack::PackId;
use crate::domain::matrix_link::{RoomAddress, RoomLink};
use crate::domain::media::AttachmentPick;
use crate::domain::message::{EditKind, EditTarget, MessageEdit, TextRevision};
use crate::domain::poll::{ChoiceMode, PollDisclosure, PollDraft};
use crate::domain::room::RoomId;
use crate::domain::room_creation::{RoomAccess, RoomDraft, RoomKind};
use crate::domain::sync::{ReauthReason, SessionLoss};
use crate::domain::uia::AccountPassword;
use crate::domain::user_info::UserId;
use crate::ports::handoff::Handoff;
use crate::ports::notifier::NoticeAction;

const PROBE_EMOTE_HEIGHT: &str = "32";

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
    ShowCallFocus,
    ShowCallGallery,
    ViewCallShare,
    ShowSelfView {
        on: bool,
    },
    ShowSharePreview {
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
    ToggleAudio,
    SeekAudio {
        ms: u64,
    },
    WindowFocus {
        focused: bool,
    },
    UserActivity {
        active: bool,
    },
    SwipeTravel {
        px: i32,
    },
    ContextPress,
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

pub enum Driven {
    Command(UiCommand),
    SessionLoss(SessionLoss),
    Poke(Poke),
    NotificationClick {
        room_id: RoomId,
        token: Option<String>,
        action: NoticeAction,
    },
    HandOff(Handoff),
}

impl fmt::Display for Driven {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(cmd) => write!(f, "{cmd}"),
            Self::SessionLoss(SessionLoss::Expired) => f.write_str("SessionExpired"),
            Self::SessionLoss(SessionLoss::Suspended(ReauthReason::SoftLogout)) => {
                f.write_str("SoftLogout")
            }
            Self::SessionLoss(SessionLoss::Suspended(ReauthReason::Refused)) => {
                f.write_str("SessionRefused")
            }
            Self::Poke(Poke::ToggleAudio) => f.write_str("ToggleAudio"),
            Self::Poke(Poke::SeekAudio(position)) => {
                write!(f, "SeekAudio({}ms)", position.as_millis())
            }
            Self::Poke(Poke::WindowFocus(focused)) => write!(f, "WindowFocus({focused})"),
            Self::Poke(Poke::UserActivity(active)) => write!(f, "UserActivity({active})"),
            Self::Poke(Poke::SwipeTravel(px)) => write!(f, "SwipeTravel({px}px)"),
            Self::HandOff(handoff) => write!(f, "HandOff(link: {})", handoff.link.is_some()),
            Self::Poke(Poke::ContextPress) => f.write_str("ContextPress"),
            Self::NotificationClick { room_id, .. } => write!(f, "ClickNotification({room_id})"),
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

fn in_room(
    explicit: Option<String>,
    selected: Selection<'_>,
    command: impl FnOnce(RoomId) -> UiCommand,
) -> Result<Driven, Rejected> {
    Ok(Driven::Command(command(room(explicit, selected)?)))
}

fn in_space(
    space_id: String,
    command: impl FnOnce(RoomId) -> UiCommand,
) -> Result<Driven, Rejected> {
    if space_id.is_empty() {
        return Err(Rejected("an empty space_id names no space".to_owned()));
    }
    Ok(Driven::Command(command(RoomId::new(space_id))))
}

fn leave_space(with_rooms: bool) -> impl FnOnce(RoomId) -> UiCommand {
    move |space_id| UiCommand::LeaveSpace {
        space_id,
        with_rooms,
    }
}

fn moderation(
    user_id: String,
    reason: String,
    command: fn(UserId, Option<String>) -> UiCommand,
) -> Result<Driven, Rejected> {
    person(user_id, |user_id| {
        command(user_id, membership_reason(reason))
    })
}

fn kick(user_id: UserId, reason: Option<String>) -> UiCommand {
    UiCommand::KickUser { user_id, reason }
}

fn ban(user_id: UserId, reason: Option<String>) -> UiCommand {
    UiCommand::BanUser { user_id, reason }
}

fn person(user_id: String, command: impl FnOnce(UserId) -> UiCommand) -> Result<Driven, Rejected> {
    if user_id.is_empty() {
        return Err(Rejected("an empty user_id names nobody".to_owned()));
    }
    Ok(Driven::Command(command(UserId::new(user_id))))
}

struct CreateRoomOp {
    name: String,
    topic: String,
    kind: String,
    access: String,
    address: String,
    encrypted: bool,
    in_space: bool,
}

fn default_room_kind() -> String {
    names::room_kind(RoomKind::Room).to_owned()
}

fn default_room_access() -> String {
    names::room_access(RoomAccess::InviteOnly).to_owned()
}

fn encrypted_by_default() -> bool {
    true
}

fn in_space_by_default() -> bool {
    true
}

fn room_draft(draft: CreateRoomOp) -> Result<UiCommand, Rejected> {
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

fn device_kind<'de, D: Deserializer<'de>>(deserializer: D) -> Result<DeviceKind, D::Error> {
    let name = String::deserialize(deserializer)?;
    names::call_device_kind_named(&name).ok_or_else(|| {
        de::Error::custom(format!(
            "{name} is not a call device kind; use microphone, speaker or camera"
        ))
    })
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
    fn into_emote(self) -> Emote {
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

fn message_edit(
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
        ProbeCommand::FilterRooms { query } => UiCommand::FilterRooms(query),
        ProbeCommand::FoldInvites => UiCommand::FoldInvites,
        ProbeCommand::UnfoldInvites => UiCommand::UnfoldInvites,
        ProbeCommand::OpenSpace { space_id } => UiCommand::OpenSpace(RoomId::new(space_id)),
        ProbeCommand::OpenSpaceIndex => UiCommand::OpenSpaceIndex,
        ProbeCommand::CloseSpaceIndex => UiCommand::CloseSpaceIndex,
        ProbeCommand::PageSpaceIndex => UiCommand::PageSpaceIndex,
        ProbeCommand::RetrySpaceIndex => UiCommand::RetrySpaceIndex,
        ProbeCommand::JoinSpaceChild { room_id } => UiCommand::JoinSpaceChild(RoomId::new(room_id)),
        ProbeCommand::OpenSpaceChild { room_id } => UiCommand::OpenSpaceChild(RoomId::new(room_id)),
        ProbeCommand::OpenInvite { room_id } => UiCommand::OpenInvite(RoomId::new(room_id)),
        ProbeCommand::CloseInvite => UiCommand::CloseInvite,
        ProbeCommand::AcceptInvite { room_id } => UiCommand::AcceptInvite(RoomId::new(room_id)),
        ProbeCommand::DeclineInvite { room_id } => UiCommand::DeclineInvite(RoomId::new(room_id)),
        ProbeCommand::OpenRemoval { room_id } => UiCommand::OpenRemoval(RoomId::new(room_id)),
        ProbeCommand::CloseRemoval => UiCommand::CloseRemoval,
        ProbeCommand::ForgetRoom { room_id } => UiCommand::ForgetRoom(RoomId::new(room_id)),
        ProbeCommand::OpenLeaveSpace { space_id } => {
            return in_space(space_id, UiCommand::OpenLeaveSpace);
        }
        ProbeCommand::CloseLeaveSpace => UiCommand::CloseLeaveSpace,
        ProbeCommand::LeaveSpace {
            space_id,
            with_rooms,
        } => return in_space(space_id, leave_space(with_rooms)),
        ProbeCommand::DeclineCall { room_id } => UiCommand::DeclineCall(RoomId::new(room_id)),
        ProbeCommand::SilenceRing => UiCommand::SilenceRing,
        ProbeCommand::OpenCallSheet { room_id, video } => {
            return in_room(room_id, selected, |room_id| UiCommand::OpenCallSheet {
                room_id,
                video,
            });
        }
        ProbeCommand::CloseCallSheet => UiCommand::CloseCallSheet,
        ProbeCommand::StartCall {
            room_id,
            video,
            microphone,
            camera,
            leaving,
        } => {
            return in_room(room_id, selected, |room_id| UiCommand::StartCall {
                room_id,
                video,
                media: MediaChoice { microphone, camera },
                leaving: leaving.map(RoomId::new),
            });
        }
        ProbeCommand::JoinCall {
            room_id,
            microphone,
            camera,
            leaving,
        } => {
            return in_room(room_id, selected, |room_id| UiCommand::JoinCall {
                room_id,
                media: MediaChoice { microphone, camera },
                leaving: leaving.map(RoomId::new),
            });
        }
        ProbeCommand::AnswerCall {
            room_id,
            camera,
            leaving,
        } => UiCommand::AnswerCall {
            room_id: RoomId::new(room_id),
            camera,
            leaving: leaving.map(RoomId::new),
        },
        ProbeCommand::LeaveCall => UiCommand::LeaveCall,
        ProbeCommand::RetryFailedCall => UiCommand::RetryFailedCall,
        ProbeCommand::DismissFailedCall => UiCommand::DismissFailedCall,
        ProbeCommand::AnnounceCallAgain => UiCommand::AnnounceCallAgain,
        ProbeCommand::SetMicrophone { on } => UiCommand::SetMicrophone(on),
        ProbeCommand::SetCamera { on } => UiCommand::SetCamera(on),
        ProbeCommand::ShareScreen { on } => UiCommand::ShareScreen(on),
        ProbeCommand::ChangeScreenShare => UiCommand::ChangeScreenShare,
        ProbeCommand::UseCallDevice { kind, key } => UiCommand::UseCallDevice {
            kind,
            key: key.filter(|key| !key.is_empty()).map(DeviceKey::new),
        },
        ProbeCommand::RefreshCallDevices => UiCommand::RefreshCallDevices,
        ProbeCommand::FocusCallTile { key } => UiCommand::FocusCallTile(key),
        ProbeCommand::ShowCallFocus => UiCommand::ShowCallFocus,
        ProbeCommand::ShowCallGallery => UiCommand::ShowCallGallery,
        ProbeCommand::ViewCallShare => UiCommand::ViewCallShare,
        ProbeCommand::ShowSelfView { on } => UiCommand::ShowSelfView(on),
        ProbeCommand::ShowSharePreview { on } => UiCommand::ShowSharePreview(on),
        ProbeCommand::JoinSuccessor { room_id } => {
            return in_room(room_id, selected, UiCommand::JoinSuccessor);
        }
        ProbeCommand::OpenRoomLink {
            room_id,
            alias,
            event_id,
            via,
        } => {
            let room = match (room_id, alias) {
                (Some(room_id), None) => RoomAddress::Id(RoomId::new(room_id)),
                (None, Some(alias)) => RoomAddress::Alias(alias),
                _ => {
                    return Err(Rejected(
                        "open_room_link takes exactly one of room_id and alias".to_owned(),
                    ));
                }
            };
            UiCommand::OpenRoomLink(RoomLink {
                room,
                event: event_id,
                via,
            })
        }
        ProbeCommand::OpenRoomFinder => UiCommand::OpenRoomFinder,
        ProbeCommand::FindRoom { address } => UiCommand::FindRoom(room_address_entry(address)),
        ProbeCommand::JoinPreviewedRoom { address } => UiCommand::JoinPreviewedRoom(address),
        ProbeCommand::RetryRoomPreview => UiCommand::RetryRoomPreview,
        ProbeCommand::CloseRoomPreview => UiCommand::CloseRoomPreview,
        ProbeCommand::OpenCreateRoom => UiCommand::OpenCreateRoom,
        ProbeCommand::OpenCreateSpace => UiCommand::OpenCreateSpace,
        ProbeCommand::CloseCreateRoom => UiCommand::CloseCreateRoom,
        ProbeCommand::CreateRoom {
            name,
            topic,
            kind,
            access,
            address,
            encrypted,
            in_space,
        } => room_draft(CreateRoomOp {
            name,
            topic,
            kind,
            access,
            address,
            encrypted,
            in_space,
        })?,
        ProbeCommand::OpenInvitePeople { room_id } => {
            return in_room(room_id, selected, UiCommand::OpenInvitePeople);
        }
        ProbeCommand::CloseInvitePeople => UiCommand::CloseInvitePeople,
        ProbeCommand::SearchPeople { query } => UiCommand::SearchPeople(query),
        ProbeCommand::RetryPeopleSearch => UiCommand::RetryPeopleSearch,
        ProbeCommand::PickInvitee { user_id } => return person(user_id, UiCommand::PickInvitee),
        ProbeCommand::UnpickInvitee { user_id } => {
            return person(user_id, UiCommand::UnpickInvitee);
        }
        ProbeCommand::SendInvites { reason } => UiCommand::SendInvites(reason),
        ProbeCommand::JoinPredecessor { room_id } => {
            return in_room(room_id, selected, UiCommand::JoinPredecessor);
        }
        ProbeCommand::OpenRoomInfo { room_id } => {
            return in_room(room_id, selected, UiCommand::OpenRoomInfo);
        }
        ProbeCommand::CloseRoomInfo => UiCommand::CloseRoomInfo,
        ProbeCommand::ShowRoomInfoPane => UiCommand::ShowRoomInfoPane,
        ProbeCommand::HideRoomInfoPane => UiCommand::HideRoomInfoPane,
        ProbeCommand::PageRoomMembers => UiCommand::PageRoomMembers,
        ProbeCommand::RetryRoomMembers => UiCommand::RetryRoomMembers,
        ProbeCommand::FilterRoomMembers { query } => UiCommand::FilterRoomMembers(query),
        ProbeCommand::SetRoomNotify { room_id, mode } => room_notify(room_id, &mode, selected)?,
        ProbeCommand::LeaveRoom { room_id } => {
            return in_room(room_id, selected, UiCommand::LeaveRoom);
        }
        ProbeCommand::OpenRoomMenu { room_id } => {
            return in_room(room_id, selected, UiCommand::OpenRoomMenu);
        }
        ProbeCommand::CloseRoomMenu => UiCommand::CloseRoomMenu,
        ProbeCommand::MarkRoomRead { room_id } => {
            return in_room(room_id, selected, UiCommand::MarkRoomRead);
        }
        ProbeCommand::CopyRoomLink { room_id } => {
            return in_room(room_id, selected, UiCommand::CopyRoomLink);
        }
        ProbeCommand::OpenRoomLog { room_id } => {
            return in_room(room_id, selected, UiCommand::OpenRoomLog);
        }
        ProbeCommand::CloseRoomLog => UiCommand::CloseRoomLog,
        ProbeCommand::OpenUserInfo { user_id } => return person(user_id, UiCommand::OpenUserInfo),
        ProbeCommand::CloseUserInfo => UiCommand::CloseUserInfo,
        ProbeCommand::RetryUserInfo => UiCommand::RetryUserInfo,
        ProbeCommand::MessageUser { user_id } => return person(user_id, UiCommand::MessageUser),
        ProbeCommand::VerifyUser { user_id } => return person(user_id, UiCommand::VerifyUser),
        ProbeCommand::IgnoreUser { user_id } => return person(user_id, UiCommand::IgnoreUser),
        ProbeCommand::UnignoreUser { user_id } => return person(user_id, UiCommand::UnignoreUser),
        ProbeCommand::KickUser { user_id, reason } => return moderation(user_id, reason, kick),
        ProbeCommand::BanUser { user_id, reason } => return moderation(user_id, reason, ban),
        ProbeCommand::UnbanUser { user_id } => return person(user_id, UiCommand::UnbanUser),
        ProbeCommand::AcceptIdentity { user_id } => {
            return person(user_id, UiCommand::AcceptIdentity);
        }
        ProbeCommand::WithdrawVerification { user_id } => {
            return person(user_id, UiCommand::WithdrawVerification);
        }
        ProbeCommand::AcknowledgeIdentityReset => UiCommand::AcknowledgeIdentityReset,
        ProbeCommand::OpenEncryption => UiCommand::OpenEncryption,
        ProbeCommand::CloseEncryption => UiCommand::CloseEncryption,
        ProbeCommand::ShowEncryptionOverview => UiCommand::ShowEncryptionOverview,
        ProbeCommand::EnterRecoveryKey => UiCommand::EnterRecoveryKey,
        ProbeCommand::UseRecoveryKey { key } => UiCommand::UseRecoveryKey(RecoveryKey::new(key)),
        ProbeCommand::VerifyWithOtherSession => UiCommand::VerifyWithOtherSession,
        ProbeCommand::SetUpRecovery => UiCommand::SetUpRecovery,
        ProbeCommand::ChangeRecoveryKey => UiCommand::ChangeRecoveryKey,
        ProbeCommand::RetryEncryptionSetup => UiCommand::RetryEncryptionSetup,
        ProbeCommand::StartIdentityReset => UiCommand::StartIdentityReset,
        ProbeCommand::ResetIdentity => UiCommand::ResetIdentity,
        ProbeCommand::StartKeyExport => UiCommand::StartKeyExport,
        ProbeCommand::ExportRoomKeys { passphrase } => {
            UiCommand::ExportRoomKeys(KeyPassphrase::new(passphrase))
        }
        ProbeCommand::StartKeyImport => UiCommand::StartKeyImport,
        ProbeCommand::ImportRoomKeys { passphrase } => {
            UiCommand::ImportRoomKeys(KeyPassphrase::new(passphrase))
        }
        ProbeCommand::OpenAccount => UiCommand::OpenAccount,
        ProbeCommand::CloseAccount => UiCommand::CloseAccount,
        ProbeCommand::ShowAccountOverview => UiCommand::ShowAccountOverview,
        ProbeCommand::RenameSession { device_id, name } => {
            UiCommand::RenameSession { device_id, name }
        }
        ProbeCommand::SignOutSession { device_id } => UiCommand::SignOutSession(device_id),
        ProbeCommand::OpenAccountPage => UiCommand::OpenAccountPage(AccountPage::Account),
        ProbeCommand::OpenSessionsPage => UiCommand::OpenAccountPage(AccountPage::Sessions),
        ProbeCommand::StartPasswordChange => UiCommand::StartPasswordChange,
        ProbeCommand::ChangePassword {
            password,
            sign_out_others,
        } => UiCommand::ChangePassword {
            password: AccountPassword::new(password),
            sign_out_others,
        },
        ProbeCommand::StartDeactivation => UiCommand::StartDeactivation,
        ProbeCommand::DeactivateAccount { erase } => UiCommand::DeactivateAccount { erase },
        ProbeCommand::AnswerAuthPassword { password } => {
            UiCommand::AnswerAuthPassword(AccountPassword::new(password))
        }
        ProbeCommand::ContinueAuth => UiCommand::ContinueAuth,
        ProbeCommand::ReopenAuthPage => UiCommand::ReopenAuthPage,
        ProbeCommand::CancelAuth => UiCommand::CancelAuth,
        ProbeCommand::ConfirmLogout => UiCommand::ConfirmLogout,
        ProbeCommand::CancelLogout => UiCommand::CancelLogout,
        ProbeCommand::SendMessage {
            room_id,
            body,
            emotes,
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
                emotes: emotes.into_iter().map(EmoteDto::into_emote).collect(),
            },
        },
        ProbeCommand::EditMessage {
            room_id,
            event_id,
            local_id,
            caption,
            body,
            original,
            emotes,
        } => UiCommand::EditMessage {
            room_id: room(room_id, selected)?,
            edit: message_edit(
                EditTarget::of(event_id, local_id),
                caption,
                body,
                original,
                emotes.into_iter().map(EmoteDto::into_emote).collect(),
            )?,
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
            caption_emotes,
            as_document,
            reply_to,
        } => UiCommand::SendAttachment {
            room_id: room(room_id, selected)?,
            caption: ComposerText {
                text: caption,
                emotes: caption_emotes
                    .into_iter()
                    .map(EmoteDto::into_emote)
                    .collect(),
            },
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
        ProbeCommand::PreviousPinned { event_id } => UiCommand::PreviousPinned { event_id },
        ProbeCommand::NextPinned { event_id } => UiCommand::NextPinned { event_id },
        ProbeCommand::CopyMessageLink { event_id } => UiCommand::CopyMessageLink { event_id },
        ProbeCommand::OpenEventSource { event_id } => UiCommand::OpenEventSource { event_id },
        ProbeCommand::CloseEventSource => UiCommand::CloseEventSource,
        ProbeCommand::OpenReaders { event_id } => UiCommand::OpenReaders { event_id },
        ProbeCommand::CloseReaders => UiCommand::CloseReaders,
        ProbeCommand::PageReaders => UiCommand::PageReaders,
        ProbeCommand::SuggestMentions {
            room_id,
            query,
            recent,
        } => UiCommand::SuggestMentions {
            room_id: room(room_id, selected)?,
            query,
            recent: recent.into_iter().map(UserId::new).collect(),
        },
        ProbeCommand::EndMentions => UiCommand::EndMentions,
        ProbeCommand::PinMessage { event_id } => UiCommand::PinMessage { event_id },
        ProbeCommand::UnpinMessage { event_id } => UiCommand::UnpinMessage { event_id },
        ProbeCommand::DeleteMessage { event_id } => UiCommand::DeleteMessage { event_id },
        ProbeCommand::ToggleReaction { event_id, key } => {
            UiCommand::ToggleReaction { event_id, key }
        }
        ProbeCommand::ReactWithEmote {
            event_id,
            pack,
            shortcode,
        } => UiCommand::ReactWithEmote {
            event_id,
            pack: PackId::new(pack),
            shortcode,
        },
        ProbeCommand::SetPackGlobal { pack, global } => UiCommand::SetPackGlobal {
            pack: PackId::new(pack),
            global,
        },
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
        ProbeCommand::UserActivity { active } => {
            return Ok(Driven::Poke(Poke::UserActivity(active)));
        }
        ProbeCommand::SwipeTravel { px } => return Ok(Driven::Poke(Poke::SwipeTravel(px))),
        ProbeCommand::ContextPress => return Ok(Driven::Poke(Poke::ContextPress)),
        ProbeCommand::ClickNotification {
            room_id,
            token,
            action,
        } => {
            let action = match action.as_deref() {
                None | Some("open") => NoticeAction::Open,
                Some("decline") => NoticeAction::Decline,
                Some("answer") => NoticeAction::Answer,
                Some(other) => {
                    return Err(Rejected(format!("{other} is not a notification action")));
                }
            };
            return Ok(Driven::NotificationClick {
                room_id: RoomId::new(room_id),
                token,
                action,
            });
        }
        ProbeCommand::OpenLink { url } => {
            link_command(&url).ok_or_else(|| Rejected(format!("{url} is not a link U2DM opens")))?
        }
        ProbeCommand::OpenExternalLink { url } => UiCommand::OpenExternalLink(
            matrix_link(&url).ok_or_else(|| Rejected(format!("{url} is not a Matrix link")))?,
        ),
        ProbeCommand::HandOff { link, token } => {
            return Ok(Driven::HandOff(Handoff { link, token }));
        }
        ProbeCommand::AcceptVerification => UiCommand::AcceptVerification,
        ProbeCommand::RejectVerification => UiCommand::RejectVerification,
        ProbeCommand::ConfirmVerification => UiCommand::ConfirmVerification,
        ProbeCommand::DismissVerification => UiCommand::DismissVerification,
        ProbeCommand::SessionExpired => return Ok(Driven::SessionLoss(SessionLoss::Expired)),
        ProbeCommand::SoftLogout => {
            return Ok(Driven::SessionLoss(SessionLoss::Suspended(
                ReauthReason::SoftLogout,
            )));
        }
        ProbeCommand::SessionRefused => {
            return Ok(Driven::SessionLoss(SessionLoss::Suspended(
                ReauthReason::Refused,
            )));
        }
        ProbeCommand::DismissToast => UiCommand::DismissToast,
        ProbeCommand::Logout => UiCommand::Logout,
        ProbeCommand::Quit => UiCommand::Quit,
    }))
}
