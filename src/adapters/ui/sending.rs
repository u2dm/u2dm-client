use super::session::with_session;
use crate::adapters::draft_size::RoomFacts;
use crate::adapters::markdown::RoomMentions;
use crate::domain::event_size::{Encryption, EventIds};
use crate::domain::message::MessagePermissions;
use crate::domain::room::RoomId;

pub struct SendingRoom {
    room_id: String,
    encryption: Encryption,
    event_ids: EventIds,
    room_mentions: RoomMentions,
}

pub fn follow_room(
    room_id: &RoomId,
    encrypted: bool,
    event_ids: EventIds,
    messages: MessagePermissions,
) {
    with_session(|session| {
        session.sending = Some(SendingRoom {
            room_id: room_id.to_string(),
            encryption: Encryption::when(encrypted),
            event_ids,
            room_mentions: RoomMentions::when(messages.notify_room),
        });
    });
}

pub struct SendingContext {
    room_id: String,
    sender: String,
    device_id: String,
    encryption: Encryption,
    event_ids: EventIds,
    room_mentions: RoomMentions,
}

impl SendingContext {
    pub fn of(room_id: &str) -> Self {
        Self::matching(Some(room_id))
    }

    pub fn selected() -> Self {
        Self::matching(None)
    }

    fn matching(room_id: Option<&str>) -> Self {
        with_session(|session| {
            let lifecycle = session.snapshot.as_ref().map(|view| &view.lifecycle);
            let room = session
                .sending
                .as_ref()
                .filter(|room| room_id.is_none_or(|id| id == room.room_id));
            Self {
                room_id: room_id
                    .map(str::to_owned)
                    .or_else(|| room.map(|room| room.room_id.clone()))
                    .unwrap_or_default(),
                sender: lifecycle
                    .map(|view| view.user_id.clone())
                    .unwrap_or_default(),
                device_id: lifecycle
                    .map(|view| view.device_id.clone())
                    .unwrap_or_default(),
                encryption: room.map_or(Encryption::Megolm, |room| room.encryption),
                event_ids: room.map_or(EventIds::WithDomain, |room| room.event_ids),
                room_mentions: room.map_or(RoomMentions::Allowed, |room| room.room_mentions),
            }
        })
    }

    pub fn facts(&self) -> RoomFacts<'_> {
        RoomFacts {
            room_id: &self.room_id,
            sender: &self.sender,
            device_id: &self.device_id,
            encryption: self.encryption,
            event_ids: self.event_ids,
            room_mentions: self.room_mentions,
        }
    }
}
