use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaKind {
    Photo,
    Sticker,
    Video,
}

#[derive(Debug)]
pub enum AttachmentPick {
    Media,
    Document,
    Pasted(PastedMedia),
}

#[derive(Debug)]
pub enum PastedMedia {
    File(PathBuf),
    Image(PastedImage),
}

#[derive(Debug)]
pub struct PastedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl PastedImage {
    const RGBA_BYTES: usize = 4;

    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        let expected = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?
            .checked_mul(Self::RGBA_BYTES)?;
        (rgba.len() == expected).then_some(Self {
            width,
            height,
            rgba,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedAttachment {
    pub path: PathBuf,
    pub filename: String,
    pub mimetype: String,
    pub size: u64,
    pub dimensions: Option<(u32, u32)>,
    pub duration: Option<Duration>,
    pub poster: Option<PathBuf>,
    pub waveform: Option<Waveform>,
}

impl PickedAttachment {
    pub fn is_image(&self) -> bool {
        self.mimetype.starts_with("image/")
    }

    pub fn is_video(&self) -> bool {
        self.mimetype.starts_with("video/")
    }

    pub fn is_audio(&self) -> bool {
        self.mimetype.starts_with("audio/")
    }

    pub fn preview_path(&self) -> Option<&PathBuf> {
        if self.is_video() {
            return self.poster.as_ref();
        }
        self.is_image().then_some(&self.path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingAttachment {
    pub picked: PickedAttachment,
    pub caption: Option<String>,
    pub as_document: bool,
    pub reply_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentKey(String);

impl ContentKey {
    pub fn new(key: String) -> Self {
        Self(key)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageMeta {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub mimetype: Option<String>,
    pub filename: Option<String>,
    pub thumbnail: Option<ContentKey>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoMeta {
    pub image: ImageMeta,
    pub duration: Option<Duration>,
    pub size: Option<u64>,
}

pub const WAVEFORM_PEAK: u16 = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioKind {
    Voice,
    Track,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waveform(Vec<u16>);

impl Waveform {
    pub fn from_amplitudes(amplitudes: impl IntoIterator<Item = u16>) -> Option<Self> {
        let amplitudes: Vec<u16> = amplitudes
            .into_iter()
            .map(|amplitude| amplitude.min(WAVEFORM_PEAK))
            .collect();
        (!amplitudes.is_empty()).then_some(Self(amplitudes))
    }

    pub fn from_levels(levels: &[f32]) -> Option<Self> {
        let loudest = levels.iter().copied().fold(0.0, f32::max);
        if loudest <= 0.0 {
            return None;
        }
        Self::from_amplitudes(levels.iter().map(|level| {
            let scaled = (level.max(0.0) / loudest * f32::from(WAVEFORM_PEAK)).round();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let amplitude = scaled as u16;
            amplitude
        }))
    }

    pub fn amplitudes(&self) -> &[u16] {
        &self.0
    }

    pub fn levels(&self) -> Vec<f32> {
        self.0
            .iter()
            .map(|amplitude| f32::from(*amplitude) / f32::from(WAVEFORM_PEAK))
            .collect()
    }

    pub fn resampled(&self, count: usize) -> Vec<f32> {
        let loudest = f32::from(self.0.iter().copied().max().unwrap_or(0).max(1));
        let available = self.0.len();
        (0..count)
            .map(|bar| {
                let start = bar * available / count;
                let end = ((bar + 1) * available / count).max(start + 1);
                let peak = self
                    .0
                    .get(start..end.min(available))
                    .and_then(|bucket| bucket.iter().copied().max())
                    .unwrap_or(0);
                f32::from(peak) / loudest
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaRendition {
    #[allow(dead_code)]
    Thumbnail,
    FullFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveformNeed {
    Skip,
    Compute,
}

impl WaveformNeed {
    pub fn of(meta: &AudioMeta) -> Self {
        match (meta.kind, &meta.waveform) {
            (AudioKind::Voice, None) => Self::Compute,
            _ => Self::Skip,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioMeta {
    pub kind: AudioKind,
    pub file: ContentKey,
    pub filename: String,
    pub mimetype: Option<String>,
    pub duration: Option<Duration>,
    pub size: Option<u64>,
    pub waveform: Option<Waveform>,
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
