mod history;
mod lists;
mod people;
mod sent;
mod spaces;

use std::collections::HashMap;
use std::fs;
use std::sync::OnceLock;

pub(super) use self::history::{
    latest_pinnable, messages, own_member_name, own_sender_name, pinned_message, pinned_messages,
};
pub(super) use self::lists::{
    Arrived, Created, Removed, RoomOverrides, alias_localparts, created_about, created_room_id,
    invites_with, removals_with, rooms_with, started_chat_id,
};
pub(super) use self::people::{
    direct_room_with, directory_people, guest_user, named_member, names_in, person_name, profile,
    readers, role_of, roster,
};
pub(super) use self::sent::{
    arrived_message, body_preview, minutes_before, newest_other_sender, own_attachment,
    own_composition, own_message, own_poll, own_sticker, own_verification_request, posted,
    sender_of, sent_text, sticker_asset_in,
};
pub(super) use self::spaces::{
    alias_target, event_link, is_member, is_unjoined_avatar, predecessor_of, room_about, room_card,
    room_link_of, space_children, spaces_with, successor_of,
};
use super::dto::DemoData;
use super::media;
use crate::domain::auth::Session;
use crate::domain::image_pack::{ImagePack, PackImage, PackOrigin};
use crate::domain::room::RoomId;

const UNKNOWN_SENDER: &str = "@member:matrix.org";

const OWN_NAME_IN_TIMELINES: &str = "You";

pub(super) const MINUTE_MS: u64 = 60_000;

static DATA: OnceLock<DemoData> = OnceLock::new();

static LOAD_ERROR: OnceLock<String> = OnceLock::new();

fn data() -> &'static DemoData {
    DATA.get_or_init(load)
}

fn load() -> DemoData {
    let path = media::data_path();
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            tracing::error!("demo data {} could not be read: {e}", path.display());
            drop(LOAD_ERROR.set(format!("{} could not be read: {e}", path.display())));
            return DemoData::default();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(data) => data,
        Err(e) => {
            tracing::error!("demo data {} is not valid: {e}", path.display());
            drop(LOAD_ERROR.set(format!("{} is not valid: {e}", path.display())));
            DemoData::default()
        }
    }
}

pub fn load_error() -> Option<&'static str> {
    LOAD_ERROR.get().map(String::as_str)
}

pub fn source_path() -> String {
    media::data_path().display().to_string()
}

pub fn counts() -> (usize, usize, usize) {
    let data = data();
    (data.rooms.len(), data.spaces.len(), data.timelines.len())
}

pub fn own_user() -> &'static str {
    &data().session.user_id
}

pub fn session() -> Session {
    data().session.to_session()
}

pub fn room_is_encrypted(room_id: &RoomId) -> bool {
    data()
        .rooms
        .iter()
        .any(|room| room.id == room_id.as_ref() && room.encrypted)
}

pub fn image_packs(room_id: &RoomId, shared: &HashMap<String, bool>) -> Vec<ImagePack> {
    data()
        .sticker_packs
        .iter()
        .map(|pack| (pack, pack.origin(shared.get(&pack.id).copied())))
        .filter(|(pack, origin)| *origin != PackOrigin::Room || pack.is_in_room(room_id))
        .map(|(pack, origin)| pack.to_pack(origin))
        .collect()
}

pub fn is_room_pack(pack_id: &str) -> bool {
    data()
        .sticker_packs
        .iter()
        .any(|pack| pack.id == pack_id && pack.origin(None) != PackOrigin::Personal)
}

pub fn sticker_image(pack_id: &str, shortcode: &str) -> Option<PackImage> {
    let pack = data()
        .sticker_packs
        .iter()
        .find(|pack| pack.id == pack_id)?;
    pack.to_pack(pack.origin(None))
        .images
        .into_iter()
        .find(|image| image.shortcode == shortcode)
}

pub fn pronouns(user_id: &str) -> Vec<String> {
    data().pronouns.get(user_id).cloned().unwrap_or_default()
}
