use std::sync::Arc;

string_id! {
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub struct PackId(String);
}

derefs_to_str!(PackId);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageUsage {
    Emoticon,
    Sticker,
    Both,
}

impl ImageUsage {
    pub fn named(emoticon: bool, sticker: bool) -> Option<Self> {
        match (emoticon, sticker) {
            (true, true) => Some(Self::Both),
            (true, false) => Some(Self::Emoticon),
            (false, true) => Some(Self::Sticker),
            (false, false) => None,
        }
    }

    pub fn resolved(image: Option<Self>, pack: Option<Self>) -> Self {
        image.or(pack).unwrap_or(Self::Both)
    }

    pub fn offers_emoticon(self) -> bool {
        matches!(self, Self::Emoticon | Self::Both)
    }

    pub fn offers_sticker(self) -> bool {
        matches!(self, Self::Sticker | Self::Both)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackImage {
    pub shortcode: String,
    pub body: String,
    pub mxc: String,
    pub usage: ImageUsage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackTitle {
    Named(String),
    Personal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackOrigin {
    Personal,
    Room,
    Global,
}

impl PackOrigin {
    pub fn room_pack(global: bool) -> Self {
        if global { Self::Global } else { Self::Room }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePack {
    pub id: PackId,
    pub title: PackTitle,
    pub origin: PackOrigin,
    pub avatar: Option<String>,
    pub images: Vec<PackImage>,
}

pub type ImagePacks = Arc<[ImagePack]>;
