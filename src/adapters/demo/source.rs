use serde_json::{Value, json};

use super::data;
use crate::domain::message::{MessageBody, RichText, TimelineMessage};
use crate::domain::room::RoomId;
use crate::domain::timeline::{EventSource, SourceEncryption};

const DEMO_DEVICE: &str = "DEMODEVICE";
const MEGOLM: &str = "m.megolm.v1.aes-sha2";

pub fn event_source(room_id: &RoomId, message: &TimelineMessage) -> Option<EventSource> {
    let event_id = message.event_id.clone()?;
    let undecryptable = matches!(message.body, MessageBody::UnableToDecrypt);
    let (event_type, content) = if undecryptable {
        ("m.room.encrypted", ciphertext())
    } else {
        (event_type(&message.body), content(&message.body))
    };
    let event = json!({
        "type": event_type,
        "event_id": event_id,
        "room_id": room_id.as_ref(),
        "sender": message.sender,
        "origin_server_ts": message.timestamp,
        "content": content,
    });
    let encryption = if undecryptable {
        SourceEncryption::Undecryptable
    } else if data::room_is_encrypted(room_id) {
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
        MessageBody::Service(_) => "m.room.member",
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
        MessageBody::Service(_) => json!({ "membership": "join" }),
        MessageBody::UnableToDecrypt => ciphertext(),
        MessageBody::Unsupported { kind, fallback } => json!({ "msgtype": kind, "body": fallback }),
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
        "url": "mxc://demo.local/media",
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
