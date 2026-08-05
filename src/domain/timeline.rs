use crate::domain::media::ThumbnailOutcome;
use crate::domain::message::TimelineMessage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnreadAnchor {
    pub row: usize,
    pub count: u32,
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
    pub event_id: Option<String>,
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
        Some(UnreadAnchor {
            row,
            count: u32::try_from(messages.len() - row).unwrap_or(u32::MAX),
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineFocus {
    Live,
    Event(String),
}

impl TimelineFocus {
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live)
    }

    pub fn target(&self) -> Option<&str> {
        match self {
            Self::Live => None,
            Self::Event(event_id) => Some(event_id),
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineStatus {
    Loading,
    LoadingUnread,
    LoadingFocus,
    Ready,
    Failed { retryable: bool },
    Disconnected,
}

#[derive(Debug, Clone, Default)]
pub struct PaginationState {
    pub backwards_loading: bool,
    pub forwards_loading: bool,
}

#[derive(Debug, Clone)]
pub enum TimelineUpdate {
    Patch(Box<TimelinePatch>),
    ResolvingUnread,
    Pagination {
        direction: PaginationDirection,
        outcome: PaginationOutcome,
    },
    JumpOutcome {
        event_id: String,
        target: JumpTarget,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpTarget {
    Row(usize),
    NotRenderable,
    NotLoaded,
}

impl TimelineUpdate {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Patch(patch) => patch.label(),
            Self::ResolvingUnread => "ResolvingUnread",
            Self::Pagination { .. } => "Pagination",
            Self::JumpOutcome { .. } => "JumpOutcome",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScrollMode {
    #[default]
    FollowLive,
    PreserveAnchor,
}
