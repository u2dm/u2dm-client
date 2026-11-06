use crate::domain::emote::Emote;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditTarget {
    Sent(String),
    Queued(String),
}

impl EditTarget {
    pub fn of(event_id: String, local_id: String) -> Option<Self> {
        if !event_id.is_empty() {
            return Some(Self::Sent(event_id));
        }
        (!local_id.is_empty()).then_some(Self::Queued(local_id))
    }

    pub fn event_id(&self) -> Option<&str> {
        match self {
            Self::Sent(event_id) => Some(event_id),
            Self::Queued(_) => None,
        }
    }

    pub fn local_id(&self) -> Option<&str> {
        match self {
            Self::Sent(_) => None,
            Self::Queued(local_id) => Some(local_id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Message,
    Caption,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageEdit {
    pub target: EditTarget,
    pub kind: EditKind,
    pub body: String,
    pub original: Option<String>,
    pub emotes: Vec<Emote>,
}

pub enum TextRevision {
    Blank,
    Unchanged,
    Changed(String),
}

impl TextRevision {
    pub fn of(kind: EditKind, original: Option<&str>, edited: String) -> Self {
        let blank = edited.trim().is_empty();
        let edited = match kind {
            EditKind::Message if blank => return Self::Blank,
            EditKind::Caption if blank => String::new(),
            EditKind::Message | EditKind::Caption => edited,
        };
        if original == Some(edited.as_str()) {
            Self::Unchanged
        } else {
            Self::Changed(edited)
        }
    }
}
