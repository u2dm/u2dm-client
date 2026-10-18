use matrix_sdk::Room;
use matrix_sdk::ruma::events::room::message::MessageType;
use matrix_sdk::ruma::events::tag::TagName;

use super::preview::MessagePreview;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ServerNotices {
    Shown,
    Ignored,
}

impl ServerNotices {
    pub(super) async fn in_room(room: &Room) -> Self {
        match room.tags().await {
            Ok(Some(tags)) if tags.contains_key(&TagName::ServerNotice) => Self::Shown,
            Ok(_) => Self::Ignored,
            Err(e) => {
                tracing::warn!(
                    room_id = %room.room_id(),
                    "ignoring server notices, since the room's tags did not load: {e}"
                );
                Self::Ignored
            }
        }
    }

    pub(super) fn admit(self, msgtype: &MessageType) -> bool {
        self == Self::Shown || !matches!(msgtype, MessageType::ServerNotice(_))
    }
}

pub(super) async fn previews_ignored_notice(room: &Room, preview: &MessagePreview) -> bool {
    preview.server_notice && ServerNotices::in_room(room).await == ServerNotices::Ignored
}
