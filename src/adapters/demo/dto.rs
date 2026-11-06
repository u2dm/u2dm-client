mod messages;
mod packs;
mod rooms;

use std::collections::HashMap;

use serde::Deserialize;

use self::messages::MessageDto;
use self::packs::ImagePackDto;
pub(super) use self::rooms::{InviteDto, RoomDto, SpaceDto, UnjoinedDto, quiet_room};
use super::data;
use crate::domain::auth::Session;
use crate::domain::message::RichText;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DemoData {
    pub session: SessionDto,
    #[serde(default)]
    pub rooms: Vec<RoomDto>,
    #[serde(default)]
    pub spaces: Vec<SpaceDto>,
    #[serde(default)]
    pub timelines: HashMap<String, Vec<MessageDto>>,
    #[serde(default)]
    pub pronouns: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub sticker_packs: Vec<ImagePackDto>,
    #[serde(default)]
    pub unjoined: Vec<UnjoinedDto>,
    #[serde(default)]
    pub invites: Vec<InviteDto>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SessionDto {
    pub user_id: String,
    pub device_id: String,
    pub homeserver: String,
}

impl SessionDto {
    pub fn to_session(&self) -> Session {
        Session {
            user_id: self.user_id.clone(),
            device_id: self.device_id.clone(),
            homeserver: self.homeserver.clone(),
            access_token: "demo-access-token".to_owned(),
            refresh_token: None,
            client_id: None,
        }
    }
}

pub(super) fn rich_text(body: &str, html: Option<&str>) -> RichText {
    RichText::with_html(body.to_owned(), html.map(ToOwned::to_owned))
}

fn ago_ms(now_ms: u64, minutes: u64, days: u64) -> u64 {
    let minutes = minutes.saturating_add(days.saturating_mul(24 * 60));
    data::minutes_before(now_ms, minutes)
}
