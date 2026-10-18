use matrix_sdk::Room;
use matrix_sdk::ruma::events::room::message::{MessageType, Relation};
use matrix_sdk::ruma::events::{
    AnySyncMessageLikeEvent, AnySyncTimelineEvent, SyncMessageLikeEvent,
};
use matrix_sdk::ruma::room_version_rules::RoomVersionRules;
use matrix_sdk_ui::timeline::{RoomExt as _, TimelineBuilder, default_event_filter};

use crate::adapters::matrix::server_notices::ServerNotices;

pub(super) async fn timeline_builder(room: &Room) -> TimelineBuilder {
    let notices = ServerNotices::in_room(room).await;
    tracing::debug!(room_id = %room.room_id(), ?notices, "building the room's timeline");
    room.timeline_builder()
        .event_filter(move |event, rules| shows(event, rules, notices))
}

fn shows(event: &AnySyncTimelineEvent, rules: &RoomVersionRules, notices: ServerNotices) -> bool {
    match new_message_type(event) {
        Some(msgtype) => notices.admit(msgtype),
        None => default_event_filter(event, rules),
    }
}

fn new_message_type(event: &AnySyncTimelineEvent) -> Option<&MessageType> {
    let AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomMessage(
        SyncMessageLikeEvent::Original(message),
    )) = event
    else {
        return None;
    };
    let edits_another = matches!(message.content.relates_to, Some(Relation::Replacement(_)));
    (!edits_another).then_some(&message.content.msgtype)
}
