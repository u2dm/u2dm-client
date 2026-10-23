use base64::Engine;
use base64::alphabet::{STANDARD, URL_SAFE};
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::{DecodePaddingMode, general_purpose};
use serde_json::{Map, Value, json};

use crate::domain::call::keys::{KeyIndex, MediaKey, ReceivedKey};
use crate::domain::call::{DeviceId, MemberKey};

const LENIENT: GeneralPurposeConfig =
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent);
const STANDARD_LENIENT: GeneralPurpose = GeneralPurpose::new(&STANDARD, LENIENT);
const URL_SAFE_LENIENT: GeneralPurpose = GeneralPurpose::new(&URL_SAFE, LENIENT);

pub(super) struct KeyMessage<'a> {
    pub room_id: &'a str,
    pub device: &'a str,
    pub member_id: &'a str,
    pub index: KeyIndex,
    pub key: &'a MediaKey,
    pub sent_ms: u64,
}

pub(super) fn key_content(message: &KeyMessage<'_>) -> Value {
    let entry = json!({
        "index": message.index.get(),
        "key": general_purpose::STANDARD.encode(message.key.bytes()),
    });
    json!({
        "room_id": message.room_id,
        "member_id": message.member_id,
        "media_key": entry,
        "keys": entry,
        "member": {
            "claimed_device_id": message.device,
            "id": message.member_id,
        },
        "session": {
            "application": "m.call",
            "call_id": "",
            "scope": "m.room",
        },
        "sent_ts": message.sent_ms,
    })
}

pub(super) struct Arrival<'a> {
    pub content: &'a Map<String, Value>,
    pub sender: &'a str,
    pub sender_device: &'a str,
    pub received_ms: u64,
}

pub(super) fn read_key(arrival: &Arrival<'_>) -> Result<(String, ReceivedKey), &'static str> {
    let content = arrival.content;
    let room_id = content
        .get("room_id")
        .and_then(Value::as_str)
        .ok_or("it names no room")?;
    let claimed = content
        .get("member")
        .and_then(Value::as_object)
        .and_then(|member| member.get("claimed_device_id"))
        .and_then(Value::as_str)
        .ok_or("it claims no device")?;
    if claimed != arrival.sender_device {
        return Err("its claimed device is not the device that sent it");
    }
    let entry = ["media_key", "keys"]
        .iter()
        .find_map(|field| content.get(*field).and_then(Value::as_object))
        .ok_or("it holds no key")?;
    let index = entry
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|index| u8::try_from(index).ok())
        .ok_or("its key index is missing or past 255")?;
    let encoded = entry
        .get("key")
        .and_then(Value::as_str)
        .ok_or("its key is missing")?;
    let key = decoded(encoded)?;
    let sent_ms = content
        .get("sent_ts")
        .and_then(Value::as_u64)
        .unwrap_or(arrival.received_ms);
    Ok((
        room_id.to_owned(),
        ReceivedKey {
            member: MemberKey::new(arrival.sender, DeviceId::new(claimed)),
            index: KeyIndex::new(index),
            key,
            sent_ms,
        },
    ))
}

fn decoded(encoded: &str) -> Result<MediaKey, &'static str> {
    let bytes = STANDARD_LENIENT
        .decode(encoded)
        .or_else(|_| URL_SAFE_LENIENT.decode(encoded))
        .map_err(|_| "its key is not base64")?;
    MediaKey::received(bytes).ok_or("its key is empty")
}
