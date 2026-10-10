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
    CustomEmoji(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Surface {
    Timeline,
    StickerPicker,
}

impl MediaSlot {
    pub(super) fn surface(&self) -> Surface {
        match self {
            Self::Thumbnail(_) | Self::CustomEmoji(_) => Surface::Timeline,
            Self::StickerCell(_) => Surface::StickerPicker,
        }
    }

    pub(super) fn belongs_to_timeline(&self) -> bool {
        self.surface() == Surface::Timeline
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
    SpaceChild(String),
    Member(String),
    Reader(String),
    Mention(String),
    SelectedRoom,
    RoomInfo,
    UserInfo,
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
