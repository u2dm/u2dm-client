use std::sync::Arc;
use std::{fmt, ops};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackId(String);

impl PackId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl ops::Deref for PackId {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for PackId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickerImage {
    pub shortcode: String,
    pub body: String,
    pub mxc: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickerPack {
    pub id: PackId,
    pub title: String,
    pub images: Vec<StickerImage>,
}

pub type StickerPacks = Arc<[StickerPack]>;
