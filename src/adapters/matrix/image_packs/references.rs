use std::collections::BTreeSet;
use std::time::Instant;

use matrix_sdk::Client;
use matrix_sdk::config::RequestConfig;
use matrix_sdk::ruma::OwnedUserId;
use matrix_sdk::ruma::api::client::config::{get_global_account_data, set_global_account_data};
use matrix_sdk::ruma::events::{AnyGlobalAccountDataEventContent, GlobalAccountDataEventType};
use matrix_sdk::ruma::serde::Raw;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::scan::ROOMS_FIELD;
use super::wire::PackRoomsDto;
use super::{MatrixImagePacks, PACK_FRESHNESS, PackRef, stored_account_data};
use crate::adapters::matrix::requests::absent_as_none;
use crate::adapters::matrix::user_info::own_user;
use crate::domain::image_pack::{ImagePack, PackId};
use crate::error::{AppError, Result};

const PACK_ROOMS_TYPES: [&str; 2] = ["m.image_pack.rooms", "im.ponies.emote_rooms"];

pub(super) const ACCOUNT_PACK_TYPES: [&str; 2] = ["m.image_pack", "im.ponies.user_emotes"];

pub(super) struct UnechoedWrite {
    written_at: Instant,
    earlier: Vec<Option<Value>>,
    content: Value,
}

impl MatrixImagePacks {
    pub(super) async fn pack_references(&self, client: &Client) -> Vec<PackRef> {
        let mut references = BTreeSet::new();
        for event_type in PACK_ROOMS_TYPES {
            let stored = stored_account_data(client, event_type).await;
            let dto = match self.unechoed_write(event_type, stored.as_ref()) {
                Some(written) => PackRoomsDto::deserialize(written).ok(),
                None => stored.and_then(|raw| raw.deserialize_as_unchecked::<PackRoomsDto>().ok()),
            };
            if let Some(dto) = dto {
                references.extend(dto.references());
            }
        }
        references.into_iter().collect()
    }

    fn unechoed_write(
        &self,
        event_type: &str,
        stored: Option<&Raw<AnyGlobalAccountDataEventContent>>,
    ) -> Option<Value> {
        let stored = stored.and_then(|raw| raw.deserialize_as_unchecked::<Value>().ok());
        let mut unechoed = self.unechoed.lock().ok()?;
        let write = unechoed.get(event_type)?;
        let catching_up = write.earlier.contains(&stored);
        if catching_up && write.written_at.elapsed() < PACK_FRESHNESS {
            return Some(write.content.clone());
        }
        unechoed.remove(event_type);
        None
    }

    fn remember_write(&self, event_type: &'static str, stored: Option<Value>, content: Value) {
        let Ok(mut unechoed) = self.unechoed.lock() else {
            return;
        };
        let mut earlier: Vec<Option<Value>> = unechoed
            .remove(event_type)
            .filter(|previous| previous.written_at.elapsed() < PACK_FRESHNESS)
            .map(|previous| {
                let mut earlier = previous.earlier;
                earlier.push(Some(previous.content));
                earlier
            })
            .unwrap_or_default();
        if !earlier.contains(&stored) {
            earlier.push(stored);
        }
        unechoed.insert(
            event_type,
            UnechoedWrite {
                written_at: Instant::now(),
                earlier,
                content,
            },
        );
    }

    pub(super) fn remember_references<'a>(
        &self,
        packs: impl IntoIterator<Item = &'a (PackRef, ImagePack)>,
    ) {
        if let Ok(mut references) = self.references.lock() {
            references.extend(
                packs
                    .into_iter()
                    .map(|(reference, pack)| (pack.id.clone(), reference.clone())),
            );
        }
    }

    pub(super) fn reference_of(&self, pack: &PackId) -> Result<PackRef> {
        self.references
            .lock()
            .ok()
            .and_then(|references| references.get(pack).cloned())
            .ok_or_else(|| AppError::Other(format!("Unknown image pack {pack}")))
    }

    pub(super) async fn write_reference(&self, reference: &PackRef, global: bool) -> Result<()> {
        let _write = self
            .reference_writes
            .acquire()
            .await
            .map_err(AppError::other)?;
        let client = self.matrix.client().await?;
        let own = own_user(&client)?;
        let mut updates = Vec::new();
        for event_type in PACK_ROOMS_TYPES {
            let current = fetch_pack_rooms(&client, &own, event_type).await?;
            if let Some(content) = referencing(current, reference, global, event_type)? {
                updates.push((event_type, content));
            }
        }
        for (event_type, content) in updates {
            let stored = stored_account_data(&client, event_type)
                .await
                .and_then(|raw| raw.deserialize_as_unchecked::<Value>().ok());
            put_pack_rooms(&client, &own, event_type, &content).await?;
            self.remember_write(event_type, stored, content);
        }
        Ok(())
    }
}

async fn fetch_pack_rooms(
    client: &Client,
    own: &OwnedUserId,
    event_type: &str,
) -> Result<Option<Value>> {
    let request = get_global_account_data::v3::Request::new(
        own.clone(),
        GlobalAccountDataEventType::from(event_type),
    );
    let sent = client
        .send(request)
        .with_request_config(RequestConfig::short_retry())
        .await;
    let Some(response) = absent_as_none(sent)? else {
        return Ok(None);
    };
    serde_json::from_str(response.account_data.json().get())
        .map(Some)
        .map_err(AppError::other)
}

async fn put_pack_rooms(
    client: &Client,
    own: &OwnedUserId,
    event_type: &str,
    content: &Value,
) -> Result<()> {
    let data = Raw::new(content).map_err(AppError::other)?.cast_unchecked();
    let request = set_global_account_data::v3::Request::new_raw(
        own.clone(),
        GlobalAccountDataEventType::from(event_type),
        data,
    );
    client
        .send(request)
        .with_request_config(RequestConfig::short_retry())
        .await
        .map(drop)
        .map_err(AppError::other)
}

fn referencing(
    current: Option<Value>,
    reference: &PackRef,
    global: bool,
    event_type: &str,
) -> Result<Option<Value>> {
    let mut content = match current {
        Some(Value::Object(content)) => content,
        None if global => Map::new(),
        None => return Ok(None),
        Some(_) => return Err(unexpected_shape(event_type)),
    };
    let rooms = content
        .entry(ROOMS_FIELD)
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(rooms) = rooms else {
        return Err(unexpected_shape(event_type));
    };
    let changed = if global {
        add_reference(rooms, reference)
    } else {
        remove_reference(rooms, reference)
    }
    .ok_or_else(|| unexpected_shape(event_type))?;
    Ok(changed.then_some(Value::Object(content)))
}

fn add_reference(rooms: &mut Map<String, Value>, reference: &PackRef) -> Option<bool> {
    let keys = rooms
        .entry(reference.room.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(keys) = keys else {
        return None;
    };
    if keys.contains_key(&reference.state_key) {
        return Some(false);
    }
    keys.insert(reference.state_key.clone(), Value::Object(Map::new()));
    Some(true)
}

fn remove_reference(rooms: &mut Map<String, Value>, reference: &PackRef) -> Option<bool> {
    let room = reference.room.to_string();
    let Some(keys) = rooms.get_mut(&room) else {
        return Some(false);
    };
    let Value::Object(keys) = keys else {
        return None;
    };
    let removed = keys.remove(&reference.state_key).is_some();
    if removed && keys.is_empty() {
        rooms.remove(&room);
    }
    Some(removed)
}

fn unexpected_shape(event_type: &str) -> AppError {
    AppError::Other(format!("{event_type} is not shaped as the spec says"))
}
