use matrix_sdk::Room;
use matrix_sdk::ruma::UserId;
use matrix_sdk::ruma::events::room::power_levels::{RoomPowerLevels, UserPowerLevel};
use matrix_sdk::ruma::events::{MessageLikeEventType, StateEventType};

use crate::domain::message::MessagePermissions;
use crate::domain::poll::PollPermissions;
use crate::domain::room_info::MemberRole;
use crate::domain::user_info::ModerationPowers;

pub(super) async fn room_permissions(room: &Room) -> (PollPermissions, MessagePermissions) {
    let levels = room.power_levels_or_default().await;
    (polls(room, &levels), messages(room.own_user_id(), &levels))
}

pub(super) async fn poll_permissions(room: &Room) -> PollPermissions {
    polls(room, &room.power_levels_or_default().await)
}

pub(super) async fn message_permissions(room: &Room) -> MessagePermissions {
    messages(room.own_user_id(), &room.power_levels_or_default().await)
}

fn polls(room: &Room, levels: &RoomPowerLevels) -> PollPermissions {
    let own = room.own_user_id();
    let may_send = |event_type| levels.user_can_send_message(own, event_type);
    let delivered =
        !room.encryption_state().is_encrypted() || may_send(MessageLikeEventType::RoomEncrypted);
    PollPermissions {
        vote: delivered && may_send(MessageLikeEventType::UnstablePollResponse),
        end: delivered && may_send(MessageLikeEventType::UnstablePollEnd),
        start: delivered && may_send(MessageLikeEventType::UnstablePollStart),
    }
}

fn messages(own: &UserId, levels: &RoomPowerLevels) -> MessagePermissions {
    MessagePermissions {
        delete_own: levels.user_can_redact_own_event(own),
        delete_others: levels.user_can_redact_event_of_other(own),
        pin: levels.user_can_send_state(own, StateEventType::RoomPinnedEvents),
    }
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
