use std::fs;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use super::dto::{DemoData, RoomDto, SpaceDto};
use super::media;
use super::timeline::{self, scenario};
use crate::domain::models::{
    MessageBody, ReplyInfo, Room, RoomId, Session, Space, TimelineMessage,
};

const UNKNOWN_SENDER: &str = "@member:matrix.org";

static DATA: OnceLock<DemoData> = OnceLock::new();

fn data() -> &'static DemoData {
    DATA.get_or_init(load)
}

fn load() -> DemoData {
    let path = media::assets_dir().join("data.json");
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            tracing::error!("demo data {} could not be read: {e}", path.display());
            return DemoData::default();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(data) => data,
        Err(e) => {
            tracing::error!("demo data {} is not valid: {e}", path.display());
            DemoData::default()
        }
    }
}

pub fn own_user() -> &'static str {
    &data().session.user_id
}

pub fn session() -> Session {
    data().session.to_session()
}

pub fn rooms() -> Vec<Room> {
    let now = now_ms();
    data().rooms.iter().map(|room| room.to_room(now)).collect()
}

pub fn spaces() -> Vec<Space> {
    data().spaces.iter().map(SpaceDto::to_space).collect()
}

pub fn messages(room_id: &RoomId) -> Vec<TimelineMessage> {
    let now = now_ms();
    let mut messages = match data().timelines.get(room_id.as_ref()) {
        Some(timeline) => timeline
            .iter()
            .map(|message| message.to_message(own_user(), now))
            .collect(),
        None => last_message_only(room_id),
    };
    if scenario().history_is_long {
        messages = repeated_history(messages);
    }
    mark_first_unread(room_id, &mut messages);
    messages
}

fn repeated_history(messages: Vec<TimelineMessage>) -> Vec<TimelineMessage> {
    let mut repeated = Vec::with_capacity(messages.len() * timeline::HISTORY_COPIES);
    for copy in 0..timeline::HISTORY_COPIES {
        for message in &messages {
            let mut message = message.clone();
            message.unique_id = format!("{}-copy{copy}", message.unique_id);
            message.event_id = message
                .event_id
                .as_ref()
                .map(|id| format!("{id}-copy{copy}"));
            repeated.push(message);
        }
    }
    repeated
}

fn unread_count(room_id: &RoomId, loaded: usize) -> usize {
    if scenario().read_position_precedes_history {
        return loaded;
    }
    if scenario().history_is_long {
        return loaded * timeline::UNREAD_PORTION_NUMERATOR / timeline::UNREAD_PORTION_DENOMINATOR;
    }
    data()
        .rooms
        .iter()
        .find(|room| room.id == room_id.as_ref())
        .map_or(0, |room| usize::try_from(room.unread).unwrap_or(usize::MAX))
}

fn mark_first_unread(room_id: &RoomId, messages: &mut [TimelineMessage]) {
    let unread = unread_count(room_id, messages.len());
    if unread == 0 {
        return;
    }
    let read_up_to = messages.len().saturating_sub(unread);
    if let Some(message) = messages.iter_mut().skip(read_up_to).find(|m| !m.is_own) {
        message.is_first_unread = true;
    }
}

pub fn pronouns(user_id: &str) -> Vec<String> {
    data().pronouns.get(user_id).cloned().unwrap_or_default()
}

pub fn own_message(sequence: u64, body: &str, reply: Option<ReplyInfo>) -> TimelineMessage {
    let id = format!("demo-sent-{sequence}");
    TimelineMessage {
        unique_id: id.clone(),
        event_id: Some(id),
        sender_pronouns: Vec::new(),
        sender: own_user().to_owned(),
        sender_display_name: Some("You".to_owned()),
        sender_avatar_url: Some(own_user().to_owned()),
        body: MessageBody::Text(body.to_owned()),
        timestamp: now_ms(),
        is_own: true,
        reply,
        edited: false,
        is_first_unread: false,
    }
}

pub fn body_preview(body: &MessageBody) -> String {
    match body {
        MessageBody::Text(text) | MessageBody::Notice(text) | MessageBody::Emote(text) => {
            text.clone()
        }
        MessageBody::Image { caption, .. } => caption.clone().unwrap_or_default(),
        MessageBody::File { meta } => meta.filename.clone(),
        MessageBody::Service(_) | MessageBody::UnableToDecrypt => String::new(),
        MessageBody::Unsupported { fallback, .. } => fallback.clone(),
    }
}

pub fn sender_label(message: &TimelineMessage) -> String {
    message
        .sender_display_name
        .clone()
        .unwrap_or_else(|| message.sender.clone())
}

fn last_message_only(room_id: &RoomId) -> Vec<TimelineMessage> {
    let Some(dto) = data().rooms.iter().find(|room| room.id == room_id.as_ref()) else {
        return Vec::new();
    };
    if dto.last_message.body.is_empty() {
        return Vec::new();
    }

    vec![synthesized_message(dto, &dto.to_room(now_ms()))]
}

fn synthesized_message(dto: &RoomDto, room: &Room) -> TimelineMessage {
    let (sender, display_name) = if dto.last_message.own {
        (own_user().to_owned(), "You".to_owned())
    } else {
        (
            dto.last_message
                .sender_id
                .clone()
                .unwrap_or_else(|| UNKNOWN_SENDER.to_owned()),
            dto.last_message.sender.clone().unwrap_or_default(),
        )
    };
    let id = format!("demo-{}-last", dto.id.trim_start_matches('!'));

    TimelineMessage {
        unique_id: id.clone(),
        event_id: Some(id),
        sender_pronouns: pronouns(&sender),
        sender_avatar_url: Some(sender.clone()),
        sender,
        sender_display_name: Some(display_name),
        body: MessageBody::Text(room.last_message_body.clone()),
        timestamp: room.last_activity_ts,
        is_own: dto.last_message.own,
        reply: None,
        edited: room.last_message_edited,
        is_first_unread: false,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or_default()
}
