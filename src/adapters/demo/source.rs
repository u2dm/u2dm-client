use serde_json::{Value, json};

use super::data;
use crate::domain::message::{MessageBody, RichText, ServiceEvent, TimelineMessage};
use crate::domain::room::RoomId;
use crate::domain::timeline::{EventSource, SourceEncryption};

const DEMO_DEVICE: &str = "DEMODEVICE";
const MEGOLM: &str = "m.megolm.v1.aes-sha2";
const DEMO_MEDIA: &str = "mxc://demo.local/media";

pub fn event_source(room_id: &RoomId, message: &TimelineMessage) -> Option<EventSource> {
    let event_id = message.event_id.clone()?;
    let undecryptable = matches!(message.body, MessageBody::UnableToDecrypt);
    let (event_type, content) = if undecryptable {
        ("m.room.encrypted", ciphertext())
    } else {
        (event_type(&message.body), content(&message.body))
    };
    let mut event = json!({
        "type": event_type,
        "event_id": event_id,
        "room_id": room_id.as_ref(),
        "sender": message.sender,
        "origin_server_ts": message.timestamp,
        "content": content,
    });
    if let MessageBody::Service(service) = &message.body
        && let Some(key) = state_key(service, &message.sender)
        && let Some(fields) = event.as_object_mut()
    {
        fields.insert("state_key".to_owned(), Value::from(key));
    }
    let encryption = if undecryptable {
        SourceEncryption::Undecryptable
    } else if data::room_is_encrypted(room_id) && !is_state_event(&message.body) {
        SourceEncryption::Decrypted {
            details: pretty(&json!({
                "sender": message.sender,
                "sender_device": DEMO_DEVICE,
                "algorithm_info": { "MegolmV1AesSha2": { "curve25519_key": "demo" } },
                "verification_state": "Verified",
            })),
        }
    } else {
        SourceEncryption::Plain
    };
    Some(EventSource {
        event_id,
        json: pretty(&event),
        edit_json: None,
        encryption,
    })
}

fn event_type(body: &MessageBody) -> &'static str {
    match body {
        MessageBody::Sticker { .. } => "m.sticker",
        MessageBody::Poll(_) => "org.matrix.msc3381.poll.start",
        MessageBody::Service(service) => service_type(service),
        MessageBody::UnableToDecrypt => "m.room.encrypted",
        MessageBody::Text(_)
        | MessageBody::Notice(_)
        | MessageBody::Emote(_)
        | MessageBody::Image { .. }
        | MessageBody::Video { .. }
        | MessageBody::Audio { .. }
        | MessageBody::File { .. }
        | MessageBody::Unsupported { .. } => "m.room.message",
    }
}

fn content(body: &MessageBody) -> Value {
    match body {
        MessageBody::Text(text) => text_content("m.text", text),
        MessageBody::Notice(text) => text_content("m.notice", text),
        MessageBody::Emote(text) => text_content("m.emote", text),
        MessageBody::Image { caption, .. } => media_content("m.image", caption.as_ref()),
        MessageBody::Video { caption, .. } => media_content("m.video", caption.as_ref()),
        MessageBody::Audio { caption, .. } => media_content("m.audio", caption.as_ref()),
        MessageBody::File { .. } => json!({ "msgtype": "m.file", "body": "file" }),
        MessageBody::Sticker { alt, .. } => json!({ "body": alt }),
        MessageBody::Poll(poll) => {
            json!({ "org.matrix.msc3381.poll.start": { "question": { "body": poll.question } } })
        }
        MessageBody::Service(service) => service_content(service),
        MessageBody::UnableToDecrypt => ciphertext(),
        MessageBody::Unsupported { kind, fallback } => json!({ "msgtype": kind, "body": fallback }),
    }
}

fn is_state_event(body: &MessageBody) -> bool {
    matches!(
        body,
        MessageBody::Service(service)
            if !matches!(service, ServiceEvent::CallStarted | ServiceEvent::CallNotification)
    )
}

fn service_type(service: &ServiceEvent) -> &'static str {
    match service {
        ServiceEvent::RoomNameChanged { .. } => "m.room.name",
        ServiceEvent::RoomTopicChanged => "m.room.topic",
        ServiceEvent::RoomAvatarChanged => "m.room.avatar",
        ServiceEvent::RoomCreated => "m.room.create",
        ServiceEvent::EncryptionEnabled => "m.room.encryption",
        ServiceEvent::CallStarted => "m.call.invite",
        ServiceEvent::CallNotification => "m.rtc.notification",
        ServiceEvent::Joined
        | ServiceEvent::Left
        | ServiceEvent::Invited { .. }
        | ServiceEvent::InvitationAccepted
        | ServiceEvent::InvitationRejected
        | ServiceEvent::InvitationRevoked { .. }
        | ServiceEvent::Kicked { .. }
        | ServiceEvent::Banned { .. }
        | ServiceEvent::Unbanned { .. }
        | ServiceEvent::Knocked
        | ServiceEvent::KnockAccepted { .. }
        | ServiceEvent::DisplayNameSet { .. }
        | ServiceEvent::DisplayNameChanged { .. }
        | ServiceEvent::DisplayNameRemoved
        | ServiceEvent::AvatarChanged => "m.room.member",
    }
}

fn state_key<'a>(service: &ServiceEvent, sender: &'a str) -> Option<&'a str> {
    match service {
        ServiceEvent::Joined
        | ServiceEvent::Left
        | ServiceEvent::InvitationAccepted
        | ServiceEvent::InvitationRejected
        | ServiceEvent::Knocked
        | ServiceEvent::DisplayNameSet { .. }
        | ServiceEvent::DisplayNameChanged { .. }
        | ServiceEvent::DisplayNameRemoved
        | ServiceEvent::AvatarChanged => Some(sender),
        ServiceEvent::RoomNameChanged { .. }
        | ServiceEvent::RoomTopicChanged
        | ServiceEvent::RoomAvatarChanged
        | ServiceEvent::RoomCreated
        | ServiceEvent::EncryptionEnabled => Some(""),
        ServiceEvent::Invited { .. }
        | ServiceEvent::InvitationRevoked { .. }
        | ServiceEvent::Kicked { .. }
        | ServiceEvent::Banned { .. }
        | ServiceEvent::Unbanned { .. }
        | ServiceEvent::KnockAccepted { .. }
        | ServiceEvent::CallStarted
        | ServiceEvent::CallNotification => None,
    }
}

fn service_content(service: &ServiceEvent) -> Value {
    match service {
        ServiceEvent::Joined
        | ServiceEvent::InvitationAccepted
        | ServiceEvent::DisplayNameRemoved => json!({ "membership": "join" }),
        ServiceEvent::Left
        | ServiceEvent::InvitationRejected
        | ServiceEvent::InvitationRevoked { .. }
        | ServiceEvent::Kicked { .. }
        | ServiceEvent::Unbanned { .. } => json!({ "membership": "leave" }),
        ServiceEvent::Invited { target } | ServiceEvent::KnockAccepted { target } => {
            json!({ "membership": "invite", "displayname": target })
        }
        ServiceEvent::Banned { .. } => json!({ "membership": "ban" }),
        ServiceEvent::Knocked => json!({ "membership": "knock" }),
        ServiceEvent::DisplayNameSet { name } | ServiceEvent::DisplayNameChanged { name } => {
            json!({ "membership": "join", "displayname": name })
        }
        ServiceEvent::AvatarChanged => json!({ "membership": "join", "avatar_url": DEMO_MEDIA }),
        ServiceEvent::RoomNameChanged { name } => json!({ "name": name }),
        ServiceEvent::RoomTopicChanged => json!({ "topic": "" }),
        ServiceEvent::RoomAvatarChanged => json!({ "url": DEMO_MEDIA }),
        ServiceEvent::RoomCreated => json!({ "room_version": "11" }),
        ServiceEvent::EncryptionEnabled => json!({ "algorithm": MEGOLM }),
        ServiceEvent::CallStarted => {
            json!({ "call_id": "demo-call", "version": "1", "lifetime": 60_000 })
        }
        ServiceEvent::CallNotification => json!({ "notification_type": "ring" }),
    }
}

fn text_content(msgtype: &str, text: &RichText) -> Value {
    match &text.html {
        Some(html) => json!({
            "msgtype": msgtype,
            "body": text.plain,
            "format": "org.matrix.custom.html",
            "formatted_body": html,
        }),
        None => json!({ "msgtype": msgtype, "body": text.plain }),
    }
}

fn media_content(msgtype: &str, caption: Option<&RichText>) -> Value {
    json!({
        "msgtype": msgtype,
        "body": caption.map_or("", |caption| caption.plain.as_str()),
        "url": DEMO_MEDIA,
    })
}

fn ciphertext() -> Value {
    json!({
        "algorithm": MEGOLM,
        "ciphertext": "AwgAEpABqOCAaP9W…",
        "device_id": DEMO_DEVICE,
        "session_id": "demo-session",
    })
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}
