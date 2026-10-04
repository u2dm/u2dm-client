use matrix_sdk::ruma::serde::Raw;
use matrix_sdk_ui::timeline::EventTimelineItem;
use serde_json::Value;

use crate::domain::timeline::{EventSource, SourceEncryption};

pub(super) fn of(event: &EventTimelineItem) -> Option<EventSource> {
    let event_id = event.event_id()?.to_string();
    let json = pretty_raw(event.original_json()?)?;
    let edit_json = event.latest_edit_json().and_then(pretty_raw);
    Some(EventSource {
        event_id,
        json,
        edit_json,
        encryption: encryption(event),
    })
}

fn encryption(event: &EventTimelineItem) -> SourceEncryption {
    if event.content().is_unable_to_decrypt() {
        return SourceEncryption::Undecryptable;
    }
    match event.encryption_info() {
        Some(info) => SourceEncryption::Decrypted {
            details: serde_json::to_string_pretty(info).unwrap_or_default(),
        },
        None => SourceEncryption::Plain,
    }
}

fn pretty_raw<T>(raw: &Raw<T>) -> Option<String> {
    let value: Value = serde_json::from_str(raw.json().get()).ok()?;
    serde_json::to_string_pretty(&value).ok()
}
