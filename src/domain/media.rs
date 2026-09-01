#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaKind {
    Photo,
    Sticker,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageMeta {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub mimetype: Option<String>,
    pub filename: Option<String>,
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
