use matrix_sdk::Room;
use matrix_sdk::ruma::events::MessageLikeEventType;

use crate::domain::poll::PollPermissions;

pub(super) async fn poll_permissions(room: &Room) -> PollPermissions {
    let levels = room.power_levels_or_default().await;
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
