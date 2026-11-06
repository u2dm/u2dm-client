use std::collections::BTreeMap;
use std::result;

use matrix_sdk::ruma::OwnedRoomId;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use super::{ImageSource, PackRef, PackSources};
use crate::domain::image_pack::{ImagePack, ImageUsage, PackId, PackImage, PackOrigin, PackTitle};

const EMOTICON_USAGE: &str = "emoticon";

const STICKER_USAGE: &str = "sticker";

const MXC_SCHEME: &str = "mxc://";

#[derive(Deserialize, Default)]
pub(super) struct PackRoomsDto {
    #[serde(default, deserialize_with = "lenient")]
    rooms: Option<BTreeMap<String, Value>>,
}

#[derive(Deserialize)]
pub(super) struct PackDto {
    #[serde(default, deserialize_with = "lenient")]
    images: Option<BTreeMap<String, Value>>,
    #[serde(default, deserialize_with = "lenient")]
    pack: Option<PackInfoDto>,
}

#[derive(Deserialize)]
struct PackImageDto {
    url: String,
    #[serde(default, deserialize_with = "lenient")]
    body: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    info: Option<Map<String, Value>>,
    #[serde(default, deserialize_with = "lenient")]
    usage: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct PackInfoDto {
    #[serde(default, deserialize_with = "lenient")]
    display_name: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    avatar_url: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    usage: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub(super) struct ScannedStateDto {
    #[serde(rename = "type")]
    pub(super) event_type: String,
    pub(super) state_key: String,
    #[serde(default)]
    pub(super) content: Value,
}

pub(super) struct PackDefaults {
    pub(super) title: PackTitle,
    pub(super) origin: PackOrigin,
    pub(super) avatar: Option<String>,
}

fn lenient<'de, D, T>(deserializer: D) -> result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(T::deserialize(Value::deserialize(deserializer)?).ok())
}

fn is_mxc(url: &str) -> bool {
    url.starts_with(MXC_SCHEME)
}

fn usage_named(kinds: Option<&Vec<String>>) -> Option<ImageUsage> {
    let kinds = kinds?;
    ImageUsage::named(
        kinds.iter().any(|kind| kind == EMOTICON_USAGE),
        kinds.iter().any(|kind| kind == STICKER_USAGE),
    )
}

impl PackRoomsDto {
    pub(super) fn references(self) -> Vec<PackRef> {
        self.rooms
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(room, keys)| {
                let room: OwnedRoomId = room.try_into().ok()?;
                let keys = BTreeMap::<String, Value>::deserialize(keys).ok()?;
                Some((room, keys))
            })
            .flat_map(|(room, keys)| {
                keys.into_keys()
                    .map(move |state_key| PackRef {
                        room: room.clone(),
                        state_key,
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

impl PackDto {
    pub(super) fn into_pack(
        self,
        id: PackId,
        defaults: PackDefaults,
    ) -> Option<(ImagePack, PackSources)> {
        let (title, avatar, pack_usage) = self
            .pack
            .map(|info| {
                let usage = usage_named(info.usage.as_ref());
                (info.display_name, info.avatar_url, usage)
            })
            .unwrap_or_default();
        let title = title
            .filter(|name| !name.is_empty())
            .map_or(defaults.title, PackTitle::Named);
        let avatar = avatar.filter(|url| is_mxc(url)).or(defaults.avatar);

        let mut images = Vec::new();
        let mut sources = PackSources::new();
        for (shortcode, image) in self.images.unwrap_or_default() {
            let Ok(image) = PackImageDto::deserialize(image) else {
                continue;
            };
            if !is_mxc(&image.url) {
                continue;
            }
            let usage = ImageUsage::resolved(usage_named(image.usage.as_ref()), pack_usage);
            let body = image
                .body
                .filter(|body| !body.is_empty())
                .unwrap_or_else(|| shortcode.clone());
            sources.insert(
                shortcode.clone(),
                ImageSource {
                    body: body.clone(),
                    url: image.url.clone(),
                    info: image.info,
                },
            );
            images.push(PackImage {
                shortcode,
                body,
                mxc: image.url,
                usage,
            });
        }

        if images.is_empty() {
            return None;
        }
        let pack = ImagePack {
            id,
            title,
            origin: defaults.origin,
            avatar,
            images,
        };
        Some((pack, sources))
    }
}
