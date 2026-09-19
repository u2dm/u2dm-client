use super::active_generation;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TimelineItemKey {
    generation: i32,
    unique_id: String,
}

impl TimelineItemKey {
    pub fn current(unique_id: &str) -> Self {
        Self {
            generation: active_generation(),
            unique_id: unique_id.to_owned(),
        }
    }

    pub fn unique_id(&self) -> &str {
        &self.unique_id
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum MediaSlot {
    Thumbnail(TimelineItemKey),
    StickerCell(String),
}

impl MediaSlot {
    pub(super) fn belongs_to_timeline(&self) -> bool {
        matches!(self, Self::Thumbnail(_))
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum AvatarSlot {
    Message(TimelineItemKey),
    Reactor {
        item: TimelineItemKey,
        user_id: String,
    },
    Room(String),
    Space(String),
    User,
    AttachmentPreview {
        pick: u64,
    },
}

impl AvatarSlot {
    pub(super) fn belongs_to_timeline(&self) -> bool {
        matches!(self, Self::Message(_) | Self::Reactor { .. })
    }

    pub(super) fn is_attachment_preview(&self) -> bool {
        matches!(self, Self::AttachmentPreview { .. })
    }
}
