use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaKind {
    Photo,
    Sticker,
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentPick {
    Media,
    Document,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedAttachment {
    pub path: PathBuf,
    pub filename: String,
    pub mimetype: String,
    pub size: u64,
    pub dimensions: Option<(u32, u32)>,
}

impl PickedAttachment {
    pub fn is_image(&self) -> bool {
        self.mimetype.starts_with("image/")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingAttachment {
    pub picked: PickedAttachment,
    pub caption: Option<String>,
    pub as_document: bool,
    pub reply_to: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageMeta {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub mimetype: Option<String>,
    pub filename: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoMeta {
    pub image: ImageMeta,
    pub duration: Option<Duration>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileMeta {
    pub filename: String,
    pub mimetype: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaFailure {
    NoSource,
    Download,
    TooLarge,
    Storage,
    Unreadable,
}

pub type MediaResult<T> = Result<T, MediaFailure>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailOutcome {
    Unchanged,
    Ready,
    Failed(MediaFailure),
}
