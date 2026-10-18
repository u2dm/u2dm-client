use matrix_sdk::Room;
use matrix_sdk::ruma::UserId;
use matrix_sdk::ruma::events::room::power_levels::{RoomPowerLevels, UserPowerLevel};
use matrix_sdk::ruma::events::{MessageLikeEventType, StateEventType};

use crate::domain::event_size::Encryption;
use crate::domain::message::MessagePermissions;
use crate::domain::poll::PollPermissions;
use crate::domain::room::SpacePowers;
use crate::domain::room_info::MemberRole;
use crate::domain::user_info::ModerationPowers;

pub(super) struct RoomPermissions {
    pub(super) polls: PollPermissions,
    pub(super) messages: MessagePermissions,
    pub(super) invite: bool,
    pub(super) calls: bool,
}

struct Sending<'a> {
    levels: &'a RoomPowerLevels,
    own: &'a UserId,
    encryption: Encryption,
}

impl<'a> Sending<'a> {
    fn in_room(room: &'a Room, levels: &'a RoomPowerLevels) -> Self {
        Self {
            levels,
            own: room.own_user_id(),
            encryption: Encryption::when(room.encryption_state().is_encrypted()),
        }
    }

    fn allows(&self, event_type: MessageLikeEventType) -> bool {
        let on_the_wire = sent_as(&event_type, self.encryption);
        self.levels.user_can_send_message(self.own, on_the_wire)
            && self.levels.user_can_send_message(self.own, event_type)
    }

    fn polls(&self) -> PollPermissions {
        PollPermissions {
            vote: self.allows(MessageLikeEventType::UnstablePollResponse),
            end: self.allows(MessageLikeEventType::UnstablePollEnd),
            start: self.allows(MessageLikeEventType::UnstablePollStart),
        }
    }

    fn messages(&self) -> MessagePermissions {
        MessagePermissions {
            send_messages: self.allows(MessageLikeEventType::RoomMessage),
            react: self.allows(MessageLikeEventType::Reaction),
            send_stickers: self.allows(MessageLikeEventType::Sticker),
            delete_own: self.levels.user_can_redact_own_event(self.own),
            delete_others: self.levels.user_can_redact_event_of_other(self.own),
            pin: self
                .levels
                .user_can_send_state(self.own, StateEventType::RoomPinnedEvents),
            notify_room: self.levels.user_can_trigger_room_notification(self.own),
        }
    }
}

fn sent_in_the_clear(event_type: &MessageLikeEventType) -> bool {
    matches!(
        event_type,
        MessageLikeEventType::Reaction | MessageLikeEventType::RoomRedaction
    )
}

fn sent_as(event_type: &MessageLikeEventType, encryption: Encryption) -> MessageLikeEventType {
    match encryption {
        Encryption::Megolm if !sent_in_the_clear(event_type) => MessageLikeEventType::RoomEncrypted,
        Encryption::Megolm | Encryption::Plain => event_type.clone(),
    }
}

pub(super) async fn room_permissions(room: &Room) -> RoomPermissions {
    let levels = room.power_levels_or_default().await;
    let own = room.own_user_id();
    let sending = Sending::in_room(room, &levels);
    RoomPermissions {
        polls: sending.polls(),
        messages: sending.messages(),
        invite: levels.user_can_invite(own),
        calls: levels.user_can_send_state(own, StateEventType::CallMember),
    }
}

pub(super) async fn space_powers(space: &Room) -> SpacePowers {
    let levels = space.power_levels_or_default().await;
    let own = space.own_user_id();
    SpacePowers {
        invite: levels.user_can_invite(own),
        add_children: levels.user_can_send_state(own, StateEventType::SpaceChild),
    }
}

pub(super) async fn may_invite(room: &Room) -> bool {
    room.power_levels_or_default()
        .await
        .user_can_invite(room.own_user_id())
}

pub(super) async fn poll_permissions(room: &Room) -> PollPermissions {
    let levels = room.power_levels_or_default().await;
    Sending::in_room(room, &levels).polls()
}

pub(super) async fn message_permissions(room: &Room) -> MessagePermissions {
    let levels = room.power_levels_or_default().await;
    Sending::in_room(room, &levels).messages()
}

pub(super) async fn may_send(
    room: &Room,
    event_type: MessageLikeEventType,
    encryption: Encryption,
) -> bool {
    let levels = room.power_levels_or_default().await;
    Sending {
        levels: &levels,
        own: room.own_user_id(),
        encryption,
    }
    .allows(event_type)
}

pub(super) fn member_role(level: UserPowerLevel) -> MemberRole {
    match level {
        UserPowerLevel::Infinite => MemberRole::Owner,
        UserPowerLevel::Int(level) => MemberRole::for_level(level.into()),
        _ => MemberRole::Member,
    }
}

pub(super) fn moderation_powers(
    levels: &RoomPowerLevels,
    own: &UserId,
    target: &UserId,
) -> ModerationPowers {
    ModerationPowers {
        kick: levels.user_can_kick_user(own, target),
        ban: levels.user_can_ban_user(own, target),
        unban: levels.user_can_unban_user(own, target),
    }
}
