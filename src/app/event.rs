mod call;
mod people;
mod rooms;
mod security;
mod session;
mod settings;

use std::path::PathBuf;

pub(super) use self::call::{CallEvent, RingDeadline};
pub(super) use self::people::{
    InvitePeopleEvent, MentionsEvent, PresenceEvent, ReadersEvent, UserInfoEvent,
};
pub(super) use self::rooms::{
    CreationSettled, DepartureEvent, MessageActionEvent, PacksEvent, PreviewEvent, RoomActionEvent,
    RoomInfoEvent, SpaceIndexEvent,
};
pub(super) use self::security::{AccountEvent, EncryptionEvent, IdentityEvent};
pub(super) use self::session::SessionEvent;
pub(super) use self::settings::{RuleSlot, SettingsEvent};
use super::invites::Answer;
use super::upgrades::UpgradeJoined;
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::domain::event_size::Oversize;
use crate::domain::media::PickedAttachment;
use crate::domain::member_name::NamedUser;
use crate::domain::message::{MessageEdit, PinnedMessage};
use crate::domain::push::PushNotice;
use crate::domain::room::RoomId;
use crate::domain::timeline::{
    AudioTrack, EventSource, MessageReaders, PaginationDirection, PaginationOutcome,
    TimelineAdvance, TimelineFocus,
};
use crate::domain::verification::VerificationEvent;
use crate::error::AppError;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ActionOutcome {
    Done,
    Failed,
}

impl ActionOutcome {
    pub(super) fn of<T>(result: Result<T, AppError>, on_failure: impl FnOnce(&AppError)) -> Self {
        match result {
            Ok(_) => Self::Done,
            Err(e) => {
                on_failure(&e);
                Self::Failed
            }
        }
    }
}

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
    SpaceIndex(SpaceIndexEvent),
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
    RoomInfo(RoomInfoEvent),
    RoomAction(RoomActionEvent),
    MessageAction(MessageActionEvent),
    UserInfo(UserInfoEvent),
    Readers(ReadersEvent),
    Mentions(MentionsEvent),
    Packs(PacksEvent),
    Identity(IdentityEvent),
    Encryption(EncryptionEvent),
    Account(AccountEvent),
    Settings(SettingsEvent),
    RoomLogGrew,
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
            Self::SpaceIndex(event) => event.label(),
            Self::InviteAnswered { .. } => "InviteAnswered",
            Self::Departure(event) => event.label(),
            Self::UpgradeJoinSettled(_) => "UpgradeJoinSettled",
            Self::Preview(event) => event.label(),
            Self::RoomCreation(_) => "RoomCreationSettled",
            Self::InvitePeople(event) => event.label(),
            Self::RoomInfo(event) => event.label(),
            Self::RoomAction(event) => event.label(),
            Self::MessageAction(event) => event.label(),
            Self::UserInfo(event) => event.label(),
            Self::Readers(event) => event.label(),
            Self::Mentions(event) => event.label(),
            Self::Packs(event) => event.label(),
            Self::Identity(event) => event.label(),
            Self::Encryption(event) => event.label(),
            Self::Account(event) => event.label(),
            Self::Settings(event) => event.label(),
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
