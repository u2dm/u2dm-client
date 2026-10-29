use std::path::PathBuf;

use super::account::AccountOutcome;
use super::calls::device_test::TestOutcome;
use super::calls::lane::LaneEvent;
use super::calls::preview::PreviewOutcome;
use super::encryption::EncryptionOutcome;
use super::establish::EstablishedSession;
use super::invites::Answer;
use super::room_info::{ActionOutcome, RosterOutcome};
use super::room_preview::JoinedRoom;
use super::space_index::{JoinOutcome, PageOutcome};
use super::space_leave::SpaceLeaveOutcome;
use super::upgrades::UpgradeJoined;
use super::user_info::{DmOutcome, ReadOutcome, VerifyOutcome};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::view::LoginActivity;
use crate::domain::auth::ServerInfo;
use crate::domain::call::devices::DeviceLists;
use crate::domain::call::hands::HandSighting;
use crate::domain::call::media::MediaEvent;
use crate::domain::call::reactions::ReactionOption;
use crate::domain::call::ring::{CallSignal, RingId};
use crate::domain::call::sfu::{GrantResult, SfuId};
use crate::domain::call::{DeviceStanding, KeySendReport, MemberKey, TransportLookup};
use crate::domain::capabilities::AccountFacts;
use crate::domain::departure::AdminStanding;
use crate::domain::encryption::{AccountEncryption, IdentityOutcome};
use crate::domain::event_size::Oversize;
use crate::domain::identity::{OwnIdentity, RoomAlert};
use crate::domain::image_pack::PackId;
use crate::domain::invite_people::{InviteResult, SearchResult};
use crate::domain::matrix_link::RoomAddress;
use crate::domain::media::PickedAttachment;
use crate::domain::member_name::NamedUser;
use crate::domain::message::{MessageEdit, PinnedMessage};
use crate::domain::presence::LookupResult;
use crate::domain::push::PushNotice;
use crate::domain::room::RoomId;
use crate::domain::room_creation::CreateResult;
use crate::domain::room_info::{Reader, RoomAbout, RosterMember};
use crate::domain::room_preview::{PreviewFailure, RoomPreview};
use crate::domain::sessions::{LogoutRisks, OwnSessions};
use crate::domain::sync::{ReauthReason, SessionLoss};
use crate::domain::timeline::{
    AudioTrack, EventSource, MessageReaders, PaginationDirection, PaginationOutcome,
    TimelineAdvance, TimelineFocus,
};
use crate::domain::user_info::{GlobalProfile, IgnoreChange, Moderation, UserId, UserTrust};
use crate::domain::verification::VerificationEvent;
use crate::error::AppError;
use crate::ports::matrix::{AuthenticatedSession, CleanupReport, PackCatalog};

#[derive(Clone, Copy)]
pub(super) enum EndReason {
    UserLogout,
    Expired,
    Deactivated,
}

pub(super) struct AttachmentPicked {
    pub(super) pick: u64,
    pub(super) room_id: RoomId,
    pub(super) outcome: Result<PickedAttachment, UserMessage>,
}

pub(super) enum AppEvent {
    Session(SessionEvent),
    Timeline(TimelineEvent),
    SpaceOrderWriteFailed {
        op: u64,
        spaces: Vec<String>,
        error: String,
    },
    VerificationFlow {
        session: u64,
        event: VerificationEvent,
    },
    VerificationActionFailed {
        session: u64,
        failure: UserMessageKind,
    },
    AttachmentPicked(Box<AttachmentPicked>),
    AttachmentSettled {
        submission: u64,
        failure: Option<UserMessage>,
    },
    AudioFetched {
        request: u64,
        outcome: Result<PathBuf, UserMessageKind>,
    },
    VideoFetched {
        request: u64,
        outcome: Result<PathBuf, UserMessageKind>,
    },
    SubmissionSettled {
        submission: i32,
        enqueue: Enqueue,
    },
    EditUnsaved {
        room_id: RoomId,
        edit: MessageEdit,
    },
    PinnedChanged {
        watch: u64,
        messages: Vec<PinnedMessage>,
    },
    TypistsChanged {
        watch: u64,
        typists: Vec<NamedUser>,
    },
    Presence(PresenceEvent),
    Call(CallEvent),
    Notified {
        session: u64,
        notice: Box<PushNotice>,
    },
    SpaceIndexPaged {
        generation: u64,
        outcome: PageOutcome,
    },
    SpaceIndexAvatarsReady {
        generation: u64,
        ready: usize,
    },
    SpaceChildJoinSettled {
        room_id: RoomId,
        name: String,
        outcome: JoinOutcome,
    },
    InviteAnswered {
        room_id: RoomId,
        name: String,
        answer: Answer,
        outcome: ActionOutcome,
    },
    Departure(DepartureEvent),
    UpgradeJoinSettled(UpgradeJoined),
    Preview(PreviewEvent),
    RoomCreation(CreationSettled),
    InvitePeople(InvitePeopleEvent),
    RoomRosterFetched {
        generation: u64,
        outcome: RosterOutcome,
    },
    RoomAboutFetched {
        generation: u64,
        about: Option<RoomAbout>,
    },
    RoomInfoAvatarsReady {
        generation: u64,
        ready: usize,
    },
    RoomAction(RoomActionEvent),
    MessageAction(MessageActionEvent),
    UserInfo(UserInfoEvent),
    Readers(ReadersEvent),
    Mentions(MentionsEvent),
    Packs(PacksEvent),
    Identity(IdentityEvent),
    Encryption(EncryptionEvent),
    Account(AccountEvent),
    RoomLogGrew,
}

pub(super) enum PreviewEvent {
    Landed {
        generation: u64,
        outcome: Result<Box<RoomPreview>, PreviewFailure>,
    },
    AvatarsReady {
        generation: u64,
        ready: usize,
    },
    JoinSettled {
        room: RoomAddress,
        name: String,
        outcome: JoinedRoom,
    },
}

impl PreviewEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::Landed { .. } => "RoomPreviewLanded",
            Self::AvatarsReady { .. } => "RoomPreviewAvatarsReady",
            Self::JoinSettled { .. } => "RoomLinkJoinSettled",
        }
    }
}

pub(super) enum PresenceEvent {
    Disabled {
        session: u64,
    },
    Looked {
        watch: u64,
        user_id: UserId,
        result: LookupResult,
    },
}

impl PresenceEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::Disabled { .. } => "PresenceDisabled",
            Self::Looked { .. } => "PresenceLooked",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RingDeadline {
    Unconfirmed,
    Expired,
}

pub(super) enum CallEvent {
    Signal {
        session: u64,
        signal: CallSignal,
    },
    RingWindow {
        ring: RingId,
        deadline: RingDeadline,
    },
    Declined {
        room_id: RoomId,
        sent: bool,
    },
    Transports {
        session: u64,
        found: Option<TransportLookup>,
    },
    Lane {
        call: u64,
        event: LaneEvent,
    },
    Media {
        call: u64,
        event: MediaEvent,
    },
    MediaFailed {
        call: u64,
        why: String,
    },
    Granted {
        call: u64,
        sfu: SfuId,
        outcome: Box<GrantResult>,
    },
    Announced {
        call: u64,
        ring: Option<RingId>,
    },
    RingOutTimeout {
        call: u64,
    },
    KeyWake {
        call: u64,
    },
    StageWake {
        call: u64,
    },
    ReactionWake {
        call: u64,
    },
    FacesReady {
        session: u64,
        ready: usize,
    },
    KeysSent {
        call: u64,
        report: KeySendReport,
    },
    Standings {
        call: u64,
        standings: Vec<(MemberKey, DeviceStanding)>,
    },
    HandSent {
        call: u64,
        membership_event: String,
        outcome: Result<String, String>,
    },
    HandLowered {
        call: u64,
        reaction: String,
        ok: bool,
    },
    HandsFetched {
        call: u64,
        sightings: Vec<HandSighting>,
    },
    ReactionSent {
        call: u64,
        membership_event: String,
        reaction: ReactionOption,
        outcome: Result<String, String>,
    },
    DevicesListed {
        listing: u64,
        lists: DeviceLists,
    },
    Preview {
        preview: u64,
        outcome: PreviewOutcome,
    },
    DeviceTest {
        test: u64,
        outcome: TestOutcome,
    },
}

impl CallEvent {
    pub(super) const fn call(&self) -> Option<u64> {
        match self {
            Self::Lane { call, .. }
            | Self::Media { call, .. }
            | Self::MediaFailed { call, .. }
            | Self::Granted { call, .. }
            | Self::Announced { call, .. }
            | Self::RingOutTimeout { call }
            | Self::KeyWake { call }
            | Self::StageWake { call }
            | Self::ReactionWake { call }
            | Self::KeysSent { call, .. }
            | Self::Standings { call, .. }
            | Self::HandSent { call, .. }
            | Self::HandLowered { call, .. }
            | Self::HandsFetched { call, .. }
            | Self::ReactionSent { call, .. } => Some(*call),
            Self::Signal { .. }
            | Self::RingWindow { .. }
            | Self::Declined { .. }
            | Self::Transports { .. }
            | Self::FacesReady { .. }
            | Self::DevicesListed { .. }
            | Self::Preview { .. }
            | Self::DeviceTest { .. } => None,
        }
    }

    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Signal { .. } => "CallSignal",
            Self::RingWindow { .. } => "CallRingWindow",
            Self::Declined { .. } => "CallDeclined",
            Self::Transports { .. } => "CallTransports",
            Self::Lane { .. } => "CallLane",
            Self::Media { .. } => "CallMedia",
            Self::MediaFailed { .. } => "CallMediaFailed",
            Self::Granted { .. } => "CallGranted",
            Self::Announced { .. } => "CallAnnounced",
            Self::RingOutTimeout { .. } => "CallRingOutTimeout",
            Self::KeyWake { .. } => "CallKeyWake",
            Self::StageWake { .. } => "CallStageWake",
            Self::ReactionWake { .. } => "CallReactionWake",
            Self::FacesReady { .. } => "CallFacesReady",
            Self::KeysSent { .. } => "CallKeysSent",
            Self::Standings { .. } => "CallStandings",
            Self::HandSent { .. } => "CallHandSent",
            Self::HandLowered { .. } => "CallHandLowered",
            Self::HandsFetched { .. } => "CallHandsFetched",
            Self::ReactionSent { .. } => "CallReactionSent",
            Self::DevicesListed { .. } => "CallDevicesListed",
            Self::Preview { .. } => "CallPreview",
            Self::DeviceTest { .. } => "CallDeviceTest",
        }
    }
}

pub(super) struct CreationSettled {
    pub(super) creation: u64,
    pub(super) outcome: CreateResult,
}

pub(super) enum InvitePeopleEvent {
    Found {
        generation: u64,
        outcome: SearchResult,
    },
    AvatarsReady {
        generation: u64,
        ready: usize,
    },
    Sent {
        batch: u64,
        outcomes: Vec<(UserId, InviteResult)>,
    },
}

impl InvitePeopleEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::Found { .. } => "PeopleFound",
            Self::AvatarsReady { .. } => "InviteeAvatarsReady",
            Self::Sent { .. } => "InvitesSent",
        }
    }
}

pub(super) enum EncryptionEvent {
    Changed {
        session: u64,
        state: AccountEncryption,
    },
    IdentitySettled {
        session: u64,
        outcome: Option<IdentityOutcome>,
    },
    ActionSettled {
        request: u64,
        outcome: EncryptionOutcome,
    },
}

impl EncryptionEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::Changed { .. } => "EncryptionChanged",
            Self::IdentitySettled { .. } => "IdentitySetupSettled",
            Self::ActionSettled { .. } => "EncryptionActionSettled",
        }
    }
}

pub(super) enum AccountEvent {
    FactsRead {
        generation: u64,
        facts: AccountFacts,
    },
    SessionsChanged {
        generation: u64,
        sessions: OwnSessions,
    },
    Settled {
        request: u64,
        outcome: AccountOutcome,
    },
    LogoutChecked {
        request: u64,
        risks: LogoutRisks,
    },
}

impl AccountEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::FactsRead { .. } => "AccountFactsRead",
            Self::SessionsChanged { .. } => "OwnSessionsChanged",
            Self::Settled { .. } => "AccountActionSettled",
            Self::LogoutChecked { .. } => "LogoutChecked",
        }
    }
}

pub(super) enum IdentityEvent {
    RoomAlerts {
        watch: u64,
        alerts: Vec<RoomAlert>,
    },
    OwnIdentity {
        session: u64,
        own: OwnIdentity,
    },
    Settled {
        request: u64,
        failure: Option<UserMessage>,
    },
}

impl IdentityEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::RoomAlerts { .. } => "IdentityAlertsChanged",
            Self::OwnIdentity { .. } => "OwnIdentityChanged",
            Self::Settled { .. } => "IdentityResolutionSettled",
        }
    }
}

pub(super) enum PacksEvent {
    CatalogLoaded {
        generation: i32,
        catalog: PackCatalog,
    },
    ImagesReady {
        generation: i32,
        ready: usize,
    },
    GlobalFailed {
        op: u64,
        pack: PackId,
    },
}

impl PacksEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::CatalogLoaded { .. } => "PackCatalogLoaded",
            Self::ImagesReady { .. } => "PackImagesReady",
            Self::GlobalFailed { .. } => "PackGlobalFailed",
        }
    }
}

pub(super) enum MentionsEvent {
    RosterLoaded {
        request: u64,
        joined_count: u64,
        roster: Result<Vec<RosterMember>, AppError>,
    },
    AvatarsReady {
        request: u64,
        ready: usize,
    },
}

impl MentionsEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::RosterLoaded { .. } => "MentionRosterLoaded",
            Self::AvatarsReady { .. } => "MentionAvatarsReady",
        }
    }
}

pub(super) enum ReadersEvent {
    Named { request: u64, readers: Vec<Reader> },
    AvatarsReady { request: u64, ready: usize },
}

impl ReadersEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::Named { .. } => "ReadersNamed",
            Self::AvatarsReady { .. } => "ReaderAvatarsReady",
        }
    }
}

pub(super) enum UserInfoEvent {
    Read {
        generation: u64,
        outcome: ReadOutcome,
    },
    PronounsFetched {
        generation: u64,
        pronouns: Vec<String>,
    },
    ProfileFetched {
        generation: u64,
        profile: Option<GlobalProfile>,
    },
    AvatarReady {
        generation: u64,
    },
    DmStarted {
        user_id: UserId,
        name: String,
        outcome: DmOutcome,
    },
    TrustChanged {
        generation: u64,
        trust: UserTrust,
    },
    VerifySettled {
        user_id: UserId,
        name: String,
        outcome: VerifyOutcome,
    },
    IgnoreSettled {
        user_id: UserId,
        name: String,
        change: IgnoreChange,
        outcome: ActionOutcome,
    },
    ModerationSettled {
        room_id: RoomId,
        user_id: UserId,
        name: String,
        action: Moderation,
        outcome: ActionOutcome,
    },
}

impl UserInfoEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::Read { .. } => "UserInfoRead",
            Self::PronounsFetched { .. } => "UserPronounsFetched",
            Self::ProfileFetched { .. } => "UserProfileFetched",
            Self::AvatarReady { .. } => "UserInfoAvatarReady",
            Self::DmStarted { .. } => "DmStarted",
            Self::TrustChanged { .. } => "UserTrustChanged",
            Self::VerifySettled { .. } => "UserVerifySettled",
            Self::IgnoreSettled { .. } => "IgnoreSettled",
            Self::ModerationSettled { .. } => "ModerationSettled",
        }
    }
}

pub(super) enum DepartureEvent {
    RemovalForgotten {
        room_id: RoomId,
        name: String,
        outcome: ActionOutcome,
    },
    StandingRead {
        generation: u64,
        standing: Option<AdminStanding>,
    },
    SpaceLeft {
        space_id: RoomId,
        name: String,
        outcome: SpaceLeaveOutcome,
    },
}

impl DepartureEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::RemovalForgotten { .. } => "RemovalForgotten",
            Self::StandingRead { .. } => "SpaceAdminStandingRead",
            Self::SpaceLeft { .. } => "SpaceLeft",
        }
    }
}

pub(super) enum RoomActionEvent {
    NotifySettled {
        request: u64,
        room_id: RoomId,
        name: String,
        outcome: ActionOutcome,
    },
    LeaveSettled {
        room_id: RoomId,
        name: String,
        outcome: ActionOutcome,
    },
    ReadSettled {
        room_id: RoomId,
        name: String,
        outcome: ActionOutcome,
    },
    LinkResolved {
        request: i32,
        name: String,
        link: Option<String>,
    },
}

impl RoomActionEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::NotifySettled { .. } => "RoomNotifySettled",
            Self::LeaveSettled { .. } => "RoomLeaveSettled",
            Self::ReadSettled { .. } => "RoomReadSettled",
            Self::LinkResolved { .. } => "RoomLinkResolved",
        }
    }
}

pub(super) enum MessageActionEvent {
    LinkResolved {
        request: i32,
        link: Option<String>,
    },
    PinSettled {
        request: u64,
        event_id: String,
        outcome: ActionOutcome,
    },
    DeletionSettled {
        event_id: String,
        outcome: ActionOutcome,
    },
}

impl MessageActionEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::LinkResolved { .. } => "MessageLinkResolved",
            Self::PinSettled { .. } => "PinChangeSettled",
            Self::DeletionSettled { .. } => "DeletionSettled",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Enqueue {
    Accepted,
    Refused,
    Oversized(Oversize),
}

impl AppEvent {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Session(event) => event.label(),
            Self::Timeline(event) => event.label(),
            Self::SpaceOrderWriteFailed { .. } => "SpaceOrderWriteFailed",
            Self::VerificationFlow { .. } => "VerificationFlow",
            Self::VerificationActionFailed { .. } => "VerificationActionFailed",
            Self::AttachmentPicked(_) => "AttachmentPicked",
            Self::AttachmentSettled { .. } => "AttachmentSettled",
            Self::AudioFetched { .. } => "AudioFetched",
            Self::VideoFetched { .. } => "VideoFetched",
            Self::SubmissionSettled { .. } => "SubmissionSettled",
            Self::EditUnsaved { .. } => "EditUnsaved",
            Self::PinnedChanged { .. } => "PinnedChanged",
            Self::TypistsChanged { .. } => "TypistsChanged",
            Self::Presence(event) => event.label(),
            Self::Call(event) => event.label(),
            Self::Notified { .. } => "Notified",
            Self::SpaceIndexPaged { .. } => "SpaceIndexPaged",
            Self::SpaceIndexAvatarsReady { .. } => "SpaceIndexAvatarsReady",
            Self::SpaceChildJoinSettled { .. } => "SpaceChildJoinSettled",
            Self::InviteAnswered { .. } => "InviteAnswered",
            Self::Departure(event) => event.label(),
            Self::UpgradeJoinSettled(_) => "UpgradeJoinSettled",
            Self::Preview(event) => event.label(),
            Self::RoomCreation(_) => "RoomCreationSettled",
            Self::InvitePeople(event) => event.label(),
            Self::RoomRosterFetched { .. } => "RoomRosterFetched",
            Self::RoomAboutFetched { .. } => "RoomAboutFetched",
            Self::RoomInfoAvatarsReady { .. } => "RoomInfoAvatarsReady",
            Self::RoomAction(event) => event.label(),
            Self::MessageAction(event) => event.label(),
            Self::UserInfo(event) => event.label(),
            Self::Readers(event) => event.label(),
            Self::Mentions(event) => event.label(),
            Self::Packs(event) => event.label(),
            Self::Identity(event) => event.label(),
            Self::Encryption(event) => event.label(),
            Self::Account(event) => event.label(),
            Self::RoomLogGrew => "RoomLogGrew",
        }
    }
}

pub(super) enum TimelineEvent {
    Advanced {
        room_id: RoomId,
        generation: i32,
        advance: TimelineAdvance,
    },
    PaginationCompleted {
        room_id: RoomId,
        generation: i32,
        direction: PaginationDirection,
        outcome: PaginationOutcome,
    },
    Refocus {
        room_id: RoomId,
        generation: i32,
        focus: TimelineFocus,
    },
    AudioLocated {
        room_id: RoomId,
        generation: i32,
        request: u64,
        track: Option<Box<AudioTrack>>,
    },
    SourceLocated {
        room_id: RoomId,
        generation: i32,
        request: u64,
        source: Option<Box<EventSource>>,
    },
    ReadersLocated {
        room_id: RoomId,
        generation: i32,
        request: u64,
        readers: Option<Box<MessageReaders>>,
    },
}

impl TimelineEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::Advanced { .. } => "TimelineAdvanced",
            Self::PaginationCompleted { .. } => "TimelinePaginationCompleted",
            Self::Refocus { .. } => "RefocusTimeline",
            Self::AudioLocated { .. } => "AudioLocated",
            Self::SourceLocated { .. } => "SourceLocated",
            Self::ReadersLocated { .. } => "ReadersLocated",
        }
    }
}

pub(super) enum SessionEvent {
    RestoreProgress(LoginActivity),
    RestoreFailed(Vec<UserMessage>),
    RestorePaused(UserMessage),
    Restored(Box<AuthenticatedSession>),
    ServerDiscovered {
        attempt: u64,
        info: Box<ServerInfo>,
    },
    AuthActivity {
        attempt: u64,
        activity: LoginActivity,
    },
    AuthRejected {
        attempt: u64,
        messages: Vec<UserMessage>,
    },
    AuthCancelled {
        attempt: u64,
    },
    LoggedIn {
        attempt: u64,
        established: Box<EstablishedSession>,
    },
    LoginUnresolved(UserMessage),
    Resumed {
        attempt: u64,
        capability: Box<AuthenticatedSession>,
    },
    ErasingLocalState {
        session: u64,
    },
    LocalStateCleared {
        session: u64,
        reason: EndReason,
        left_behind: Option<String>,
        report: CleanupReport,
    },
    TokensNotPersisted {
        session: u64,
    },
    UserAvatar {
        session: u64,
        path: Option<PathBuf>,
    },
    Suspended(ReauthReason),
    Expired,
}

impl From<SessionLoss> for SessionEvent {
    fn from(loss: SessionLoss) -> Self {
        match loss {
            SessionLoss::Suspended(reason) => Self::Suspended(reason),
            SessionLoss::Expired => Self::Expired,
        }
    }
}

impl SessionEvent {
    fn label(&self) -> &'static str {
        match self {
            Self::RestoreProgress(_) => "RestoreProgress",
            Self::RestoreFailed(_) => "RestoreFailed",
            Self::RestorePaused(_) => "RestorePaused",
            Self::Restored(_) => "Restored",
            Self::ServerDiscovered { .. } => "ServerDiscovered",
            Self::AuthActivity { .. } => "AuthActivity",
            Self::AuthRejected { .. } => "AuthRejected",
            Self::AuthCancelled { .. } => "AuthCancelled",
            Self::LoggedIn { .. } => "LoggedIn",
            Self::LoginUnresolved(_) => "LoginUnresolved",
            Self::Resumed { .. } => "Resumed",
            Self::ErasingLocalState { .. } => "ErasingLocalState",
            Self::LocalStateCleared { .. } => "LocalStateCleared",
            Self::TokensNotPersisted { .. } => "TokensNotPersisted",
            Self::UserAvatar { .. } => "UserAvatar",
            Self::Suspended(_) => "SessionSuspended",
            Self::Expired => "SessionExpired",
        }
    }
}
