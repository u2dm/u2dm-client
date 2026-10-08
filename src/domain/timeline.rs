use crate::domain::media::{AudioKind, AudioMeta, ContentKey, ThumbnailOutcome};
use crate::domain::message::{MessageEdit, TimelineMessage};
use crate::domain::poll::{PollAction, PollDraft};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnreadAnchor {
    pub row: usize,
    pub count: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum FailedSend {
    Retry,
    Discard,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Arrivals {
    pub new_messages: u32,
    pub from_others: bool,
}

impl Arrivals {
    pub fn of<'a>(messages: impl IntoIterator<Item = &'a TimelineMessage>) -> Self {
        messages
            .into_iter()
            .fold(Self::default(), |arrivals, message| Self {
                new_messages: arrivals
                    .new_messages
                    .saturating_add(u32::from(message.counts_as_unread())),
                from_others: arrivals.from_others || !message.is_own,
            })
    }

    pub fn is_silent(self) -> bool {
        self.new_messages == 0 && !self.from_others
    }
}

#[derive(Debug, Clone, strum::IntoStaticStr)]
pub enum TimelinePatch {
    Reset(Vec<TimelineMessage>),
    Append(Vec<TimelineMessage>),
    PushFront(TimelineMessage),
    PushBack(TimelineMessage),
    Insert {
        index: usize,
        message: TimelineMessage,
    },
    Set {
        index: usize,
        message: TimelineMessage,
    },
    Remove {
        index: usize,
    },
    PopFront,
    PopBack,
    Truncate {
        length: usize,
    },
    Clear,
    Batch(Vec<TimelinePatch>),
    Enrich(EnrichmentDelta),
}

#[derive(Debug, Clone)]
pub struct EnrichmentDelta {
    pub unique_id: String,
    pub thumbnail_content: Option<ContentKey>,
    pub fingerprint: u64,
    pub thumbnail: ThumbnailOutcome,
    pub avatar_mxc: Option<String>,
    pub pronouns: Option<Vec<String>>,
}

impl EnrichmentDelta {
    pub fn is_noop(&self) -> bool {
        matches!(self.thumbnail, ThumbnailOutcome::Unchanged)
            && self.avatar_mxc.is_none()
            && self.pronouns.is_none()
    }
}

impl TimelinePatch {
    pub fn label(&self) -> &'static str {
        self.into()
    }

    pub fn is_prepend(&self) -> bool {
        self.adds_at_front() && !self.adds_at_back()
    }

    pub fn opens_room(&self) -> bool {
        self.last_reset().is_some()
    }

    pub fn unread_anchor(&self) -> Option<UnreadAnchor> {
        let messages = self.last_reset()?;
        let row = messages.iter().position(|m| m.is_first_unread)?;
        let unread = messages
            .get(row..)?
            .iter()
            .filter(|message| message.counts_as_unread())
            .count();
        Some(UnreadAnchor {
            row,
            count: u32::try_from(unread).unwrap_or(u32::MAX),
        })
    }

    pub fn shifts_rows(&self) -> bool {
        match self {
            Self::PushFront(_)
            | Self::PopFront
            | Self::Insert { .. }
            | Self::Remove { .. }
            | Self::Truncate { .. } => true,
            Self::Batch(patches) => patches.iter().any(TimelinePatch::shifts_rows),
            _ => false,
        }
    }

    pub fn visit_messages_mut(&mut self, visit: &mut impl FnMut(&mut TimelineMessage)) {
        match self {
            Self::Reset(messages) | Self::Append(messages) => {
                for message in messages {
                    visit(message);
                }
            }
            Self::PushFront(message)
            | Self::PushBack(message)
            | Self::Insert { message, .. }
            | Self::Set { message, .. } => visit(message),
            Self::Batch(patches) => {
                for patch in patches {
                    patch.visit_messages_mut(visit);
                }
            }
            Self::Remove { .. }
            | Self::PopFront
            | Self::PopBack
            | Self::Truncate { .. }
            | Self::Clear
            | Self::Enrich(_) => {}
        }
    }

    fn last_reset(&self) -> Option<&[TimelineMessage]> {
        match self {
            Self::Reset(messages) => Some(messages),
            Self::Batch(patches) => patches.iter().rev().find_map(TimelinePatch::last_reset),
            _ => None,
        }
    }

    fn adds_at_front(&self) -> bool {
        match self {
            Self::PushFront(_) => true,
            Self::Insert { index, .. } => *index == 0,
            Self::Batch(patches) => patches.iter().any(TimelinePatch::adds_at_front),
            _ => false,
        }
    }

    fn adds_at_back(&self) -> bool {
        match self {
            Self::Append(_) | Self::PushBack(_) => true,
            Self::Batch(patches) => patches.iter().any(TimelinePatch::adds_at_back),
            _ => false,
        }
    }
}

#[derive(Debug)]
pub enum TimelineCommand {
    PaginateBackwards,
    PaginateForwards,
    MarkRead,
    JumpTo(String),
    ToggleReaction { event_id: String, key: String },
    VotePoll { event_id: String, answer_id: String },
    EndPoll { event_id: String },
    EditPoll { event_id: String, draft: PollDraft },
    LocateAudio { request: u64, lookup: AudioLookup },
    LocateSource { request: u64, event_id: String },
    LocateReaders { request: u64, event_id: String },
}

impl TimelineCommand {
    pub fn sends_an_event(&self) -> bool {
        match self {
            Self::ToggleReaction { .. }
            | Self::VotePoll { .. }
            | Self::EndPoll { .. }
            | Self::EditPoll { .. } => true,
            Self::PaginateBackwards
            | Self::PaginateForwards
            | Self::MarkRead
            | Self::JumpTo(_)
            | Self::LocateAudio { .. }
            | Self::LocateSource { .. }
            | Self::LocateReaders { .. } => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioLookup {
    Event(String),
    VoiceAfter(String),
}

impl AudioLookup {
    pub fn anchor(&self) -> &str {
        match self {
            Self::Event(event_id) | Self::VoiceAfter(event_id) => event_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioTrack {
    pub event_id: String,
    pub sender: String,
    pub meta: AudioMeta,
}

impl AudioTrack {
    fn of(message: TimelineMessage) -> Option<Self> {
        let meta = message.body.audio()?.clone();
        let sender = message
            .sender_display_name
            .unwrap_or_else(|| message.sender.clone());
        Some(Self {
            event_id: message.event_id?,
            sender,
            meta,
        })
    }
}

pub fn locate_audio<I>(lookup: &AudioLookup, messages: I) -> Option<AudioTrack>
where
    I: IntoIterator<Item = TimelineMessage>,
{
    let mut from_anchor = messages
        .into_iter()
        .skip_while(|message| message.event_id.as_deref() != Some(lookup.anchor()));
    match lookup {
        AudioLookup::Event(_) => from_anchor.next().and_then(AudioTrack::of),
        AudioLookup::VoiceAfter(_) => from_anchor
            .skip(1)
            .filter_map(AudioTrack::of)
            .find(|track| track.meta.kind == AudioKind::Voice),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceEncryption {
    Plain,
    Decrypted { details: String },
    Undecryptable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSource {
    pub event_id: String,
    pub json: String,
    pub edit_json: Option<String>,
    pub encryption: SourceEncryption,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageReaders {
    pub message: TimelineMessage,
    pub readers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineFocus {
    ReadPosition,
    Latest,
    Event(String),
}

impl TimelineFocus {
    pub fn is_live(&self) -> bool {
        matches!(self, Self::ReadPosition | Self::Latest)
    }

    pub fn opens_at_read_position(&self) -> bool {
        matches!(self, Self::ReadPosition)
    }

    pub fn target(&self) -> Option<&str> {
        match self {
            Self::ReadPosition | Self::Latest => None,
            Self::Event(event_id) => Some(event_id),
        }
    }
}

#[derive(Clone, Copy)]
pub enum TimelineAdvance {
    Anchored {
        count: u32,
    },
    UnreadUnresolved,
    Arrived {
        arrivals: Arrivals,
        opens_room: bool,
    },
    Focused,
}

#[derive(Debug, Clone, Copy)]
pub enum PaginationDirection {
    Backwards,
    Forwards,
}

#[derive(Debug, Clone, Copy)]
pub enum PaginationOutcome {
    Completed { hit_end: bool },
    Failed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TimelineStatus {
    #[default]
    None,
    Loading,
    LoadingUnread,
    LoadingFocus,
    Ready,
    Failed {
        retryable: bool,
    },
    Disconnected,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OlderHistory {
    #[default]
    Unknown,
    Available,
    Loading,
    Failed,
    Ended,
}

#[derive(Debug, Clone, Default)]
pub struct PaginationState {
    pub older_history: OlderHistory,
    pub forwards_loading: bool,
}

#[derive(Debug, Clone)]
pub enum TimelineUpdate {
    Patch {
        patch: Box<TimelinePatch>,
        arrivals: Arrivals,
    },
    ResolvingUnread,
    UnreadUnresolved,
    Pagination {
        direction: PaginationDirection,
        outcome: PaginationOutcome,
    },
    JumpOutcome {
        event_id: String,
        target: JumpTarget,
    },
    AudioLocated {
        request: u64,
        track: Option<Box<AudioTrack>>,
    },
    SourceLocated {
        request: u64,
        source: Option<Box<EventSource>>,
    },
    ReadersLocated {
        request: u64,
        readers: Option<Box<MessageReaders>>,
    },
    PollSendFailed(PollAction),
    EditUnsaved(MessageEdit),
    DeleteFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpTarget {
    Row(usize),
    NotRenderable,
    NotLoaded,
}

impl TimelineUpdate {
    pub fn patch(patch: TimelinePatch) -> Self {
        Self::Patch {
            patch: Box::new(patch),
            arrivals: Arrivals::default(),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Patch { patch, .. } => patch.label(),
            Self::ResolvingUnread => "ResolvingUnread",
            Self::UnreadUnresolved => "UnreadUnresolved",
            Self::Pagination { .. } => "Pagination",
            Self::JumpOutcome { .. } => "JumpOutcome",
            Self::AudioLocated { .. } => "AudioLocated",
            Self::SourceLocated { .. } => "SourceLocated",
            Self::ReadersLocated { .. } => "ReadersLocated",
            Self::PollSendFailed(_) => "PollSendFailed",
            Self::EditUnsaved(_) => "EditUnsaved",
            Self::DeleteFailed => "DeleteFailed",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScrollMode {
    #[default]
    FollowLive,
    PreserveAnchor,
}
