use serde::Deserialize;

use crate::domain::image_pack::{ImagePack, ImageUsage, PackId, PackImage, PackOrigin, PackTitle};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImagePackDto {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub origin: OriginDto,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub rooms: Vec<String>,
    #[serde(default)]
    pub usage: Option<Vec<UsageDto>>,
    #[serde(default)]
    pub images: Vec<PackImageDto>,
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OriginDto {
    Personal,
    #[default]
    Room,
    Global,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackImageDto {
    pub shortcode: String,
    #[serde(default)]
    pub body: Option<String>,
    pub asset: String,
    #[serde(default)]
    pub usage: Option<Vec<UsageDto>>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UsageDto {
    Emoticon,
    Sticker,
}

fn usage_of(kinds: Option<&Vec<UsageDto>>) -> Option<ImageUsage> {
    let kinds = kinds?;
    ImageUsage::named(
        kinds.contains(&UsageDto::Emoticon),
        kinds.contains(&UsageDto::Sticker),
    )
}

impl ImagePackDto {
    pub fn origin(&self, shared: Option<bool>) -> PackOrigin {
        match self.origin {
            OriginDto::Personal => PackOrigin::Personal,
            OriginDto::Room => PackOrigin::room_pack(shared.unwrap_or(false)),
            OriginDto::Global => PackOrigin::room_pack(shared.unwrap_or(true)),
        }
    }

    pub fn is_in_room(&self, room_id: &str) -> bool {
        self.rooms.iter().any(|id| id == room_id)
    }

    pub fn to_pack(&self, origin: PackOrigin) -> ImagePack {
        let pack_usage = usage_of(self.usage.as_ref());
        ImagePack {
            id: PackId::new(self.id.clone()),
            title: self
                .title
                .clone()
                .map_or(PackTitle::Personal, PackTitle::Named),
            origin,
            avatar: self.avatar.as_deref().map(demo_mxc),
            images: self
                .images
                .iter()
                .map(|image| image.to_image(pack_usage))
                .collect(),
        }
    }
}

impl PackImageDto {
    fn to_image(&self, pack_usage: Option<ImageUsage>) -> PackImage {
        PackImage {
            shortcode: self.shortcode.clone(),
            body: self.body.clone().unwrap_or_else(|| self.shortcode.clone()),
            mxc: demo_mxc(&self.asset),
            usage: ImageUsage::resolved(usage_of(self.usage.as_ref()), pack_usage),
        }
    }
}

fn demo_mxc(asset: &str) -> String {
    format!("mxc://demo.local/{asset}")
}
