use std::io::{Cursor, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use std::{env, fs, process};

use async_trait::async_trait;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, ImageFormat, ImageReader};
use rfd::AsyncFileDialog;
use tokio::fs as async_fs;
use tokio::task::spawn_blocking;

use crate::adapters::{container, private_fs, video};
use crate::domain::media::{AttachmentPick, PastedImage, PastedMedia, PickedAttachment, Waveform};
use crate::error::{AppError, Result};
use crate::ports::media::MediaFilePort;
use crate::util::random_hex;

const MEDIA_DIR: &str = "u2dm-media";
const MEDIA_RETENTION: Duration = Duration::from_hours(24);
const SESSION_TOKEN_BYTES: usize = 8;
const FILE_TOKEN_BYTES: usize = 16;
const ROOT_TOKEN_BYTES: usize = 16;
const ROOT_ATTEMPTS: usize = 8;
const FALLBACK_MIME: &str = "application/octet-stream";
const POSTER_MAX_EDGE: u32 = 800;
const POSTER_QUALITY: u8 = 80;
const PASTED_IMAGE_FILENAME: &str = "image.png";
const PASTED_IMAGE_COMPRESSION: CompressionType = CompressionType::Level(3);

const PICKABLE_MEDIA_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tif", "tiff", "avif", "heic", "heif",
    "mp4", "m4v", "mov", "webm", "mkv", "avi",
];

const LAUNCHER_SAFE_IMAGE_FORMATS: &[(ImageFormat, &str)] = &[
    (ImageFormat::Png, "png"),
    (ImageFormat::Jpeg, "jpg"),
    (ImageFormat::Gif, "gif"),
    (ImageFormat::WebP, "webp"),
    (ImageFormat::Bmp, "bmp"),
    (ImageFormat::Ico, "ico"),
    (ImageFormat::Tiff, "tiff"),
    (ImageFormat::Avif, "avif"),
];

pub struct DesktopMediaFiles {
    session_dir: Option<PathBuf>,
}

impl DesktopMediaFiles {
    pub fn new() -> Self {
        Self {
            session_dir: open_root().and_then(|root| open_session_dir(&root)),
        }
    }

    pub(crate) async fn describe(&self, path: &Path) -> Result<PickedAttachment> {
        describe_in(path, self.session_dir.as_deref()).await
    }

    async fn pick_from(&self, dialog: AsyncFileDialog) -> Result<Option<PickedAttachment>> {
        let Some(handle) = dialog.pick_file().await else {
            return Ok(None);
        };
        self.describe(handle.path()).await.map(Some)
    }

    async fn describe_pasted_image(&self, image: PastedImage) -> Result<PickedAttachment> {
        let session_dir = self.session_dir()?;
        let png = spawn_blocking(move || encode_png(&image))
            .await
            .map_err(|e| AppError::Other(format!("failed to encode the pasted image: {e}")))??;
        private_fs::create_dir(session_dir).await?;
        let path = session_dir.join(format!("{}.png", random_hex(FILE_TOKEN_BYTES)));
        private_fs::write_private(&path, &png).await?;
        let picked = self.describe(&path).await?;
        Ok(PickedAttachment {
            filename: PASTED_IMAGE_FILENAME.to_owned(),
            ..picked
        })
    }

    fn session_dir(&self) -> Result<&Path> {
        self.session_dir.as_deref().ok_or_else(|| {
            AppError::Other("no private directory is available to open media from".into())
        })
    }
}

#[async_trait]
impl MediaFilePort for DesktopMediaFiles {
    async fn open_media(&self, _event_id: &str, data: &[u8]) -> Result<()> {
        let session_dir = self.session_dir()?;
        let extension = launcher_safe_extension(data).ok_or(AppError::UnviewableMedia)?;
        private_fs::create_dir(session_dir).await?;
        let path = session_dir.join(format!("{}.{extension}", random_hex(FILE_TOKEN_BYTES)));
        private_fs::write_private(&path, data).await?;
        spawn_blocking(move || open::that_detached(&path))
            .await
            .map_err(|e| AppError::Other(format!("failed to launch media viewer: {e}")))??;
        Ok(())
    }

    async fn open_path(&self, path: &Path) -> Result<()> {
        let path = path.to_path_buf();
        spawn_blocking(move || open::that_detached(&path))
            .await
            .map_err(|e| AppError::Other(format!("failed to launch media viewer: {e}")))??;
        Ok(())
    }

    async fn pick_attachment(&self, pick: AttachmentPick) -> Result<Option<PickedAttachment>> {
        match pick {
            AttachmentPick::Media => {
                self.pick_from(
                    AsyncFileDialog::new()
                        .set_title("Send a photo or video")
                        .add_filter("Photos and videos", PICKABLE_MEDIA_EXTENSIONS),
                )
                .await
            }
            AttachmentPick::Document => {
                self.pick_from(AsyncFileDialog::new().set_title("Send a document"))
                    .await
            }
            AttachmentPick::Pasted(PastedMedia::File(path)) => self.describe(&path).await.map(Some),
            AttachmentPick::Pasted(PastedMedia::Image(image)) => {
                self.describe_pasted_image(image).await.map(Some)
            }
        }
    }

    async fn release_attachment(&self, picked: PickedAttachment) {
        let Some(session_dir) = self.session_dir.as_deref() else {
            return;
        };
        let written_here = [Some(picked.path), picked.poster]
            .into_iter()
            .flatten()
            .filter(|path| path.parent() == Some(session_dir));
        for path in written_here {
            if let Err(e) = async_fs::remove_file(&path).await
                && e.kind() != ErrorKind::NotFound
            {
                tracing::debug!("failed to remove {}: {e}", path.display());
            }
        }
    }

    async fn save_file(&self, default_filename: &str, data: &[u8]) -> Result<Option<String>> {
        let dialog = rfd::AsyncFileDialog::new().set_file_name(default_filename);
        let Some(file_handle) = dialog.save_file().await else {
            return Ok(None);
        };

        file_handle.write(data).await?;
        Ok(Some(file_handle.path().display().to_string()))
    }

    async fn clear_session(&self) {
        let Some(session_dir) = self.session_dir.as_deref() else {
            return;
        };
        match async_fs::remove_dir_all(session_dir).await {
            Ok(()) => tracing::debug!("cleared session media directory"),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("failed to clear session media directory: {e}"),
        }
        if let Err(e) = private_fs::create_dir(session_dir).await {
            tracing::debug!("failed to recreate session media directory: {e}");
        }
    }
}

fn encode_png(image: &PastedImage) -> Result<Vec<u8>> {
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, PASTED_IMAGE_COMPRESSION, FilterType::Adaptive)
        .write_image(
            image.rgba(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|e| AppError::Other(format!("failed to encode the pasted image: {e}")))?;
    Ok(png)
}

async fn describe_in(path: &Path, poster_dir: Option<&Path>) -> Result<PickedAttachment> {
    let metadata = async_fs::metadata(path).await?;
    if !metadata.is_file() {
        return Err(AppError::Other(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::Other("the chosen file has no usable name".into()))?
        .to_owned();

    let (mimetype, inspected) = inspect(path, sniff_mimetype(path).await, poster_dir).await;

    Ok(PickedAttachment {
        path: path.to_path_buf(),
        filename,
        mimetype,
        size: metadata.len(),
        dimensions: inspected.dimensions,
        duration: inspected.duration,
        poster: inspected.poster,
        waveform: inspected.waveform,
    })
}

#[derive(Default)]
struct Inspected {
    dimensions: Option<(u32, u32)>,
    duration: Option<Duration>,
    poster: Option<PathBuf>,
    waveform: Option<Waveform>,
}

async fn inspect(path: &Path, mimetype: String, poster_dir: Option<&Path>) -> (String, Inspected) {
    if mimetype.starts_with("image/") {
        let dimensions = probe_dimensions(path.to_path_buf()).await;
        return (
            mimetype,
            Inspected {
                dimensions,
                ..Inspected::default()
            },
        );
    }
    if mimetype.starts_with("audio/") {
        let canonical = container::canonical_audio_mime(&mimetype).to_owned();
        return (canonical, inspect_audio(path).await.unwrap_or_default());
    }
    if !mimetype.starts_with("video/") {
        return (mimetype, Inspected::default());
    }
    if let Some(inspected) = inspect_video(path, poster_dir).await {
        return (mimetype, inspected);
    }
    match container::audio_only_mime(&mimetype) {
        Some(audio) => match inspect_audio(path).await {
            Some(inspected) => (audio.to_owned(), inspected),
            None => (mimetype, Inspected::default()),
        },
        None => (mimetype, Inspected::default()),
    }
}

async fn inspect_video(path: &Path, poster_dir: Option<&Path>) -> Option<Inspected> {
    let owned = path.to_path_buf();
    let probed = spawn_blocking(move || video::probe(&owned))
        .await
        .ok()
        .flatten()?;
    let poster = match poster_dir {
        Some(dir) => write_poster(path, dir).await,
        None => None,
    };
    Some(Inspected {
        dimensions: Some((probed.width, probed.height)),
        duration: probed.duration,
        poster,
        waveform: None,
    })
}

async fn inspect_audio(path: &Path) -> Option<Inspected> {
    let owned = path.to_path_buf();
    let probed = spawn_blocking(move || video::probe_audio(&owned))
        .await
        .ok()
        .flatten()?;
    Some(Inspected {
        duration: probed.duration,
        waveform: probed.waveform,
        ..Inspected::default()
    })
}

async fn write_poster(path: &Path, dir: &Path) -> Option<PathBuf> {
    let owned = path.to_path_buf();
    let jpeg = spawn_blocking(move || video::poster_jpeg(&owned, POSTER_MAX_EDGE, POSTER_QUALITY))
        .await
        .ok()
        .flatten()?;
    private_fs::create_dir(dir).await.ok()?;
    let poster = dir.join(format!("{}.jpg", random_hex(FILE_TOKEN_BYTES)));
    private_fs::write_private(&poster, &jpeg).await.ok()?;
    Some(poster)
}

async fn sniff_mimetype(path: &Path) -> String {
    if let Some(inferred) = sniff_magic(path.to_path_buf()).await {
        return inferred;
    }
    mime_guess::from_path(path)
        .first_raw()
        .unwrap_or(FALLBACK_MIME)
        .to_owned()
}

async fn sniff_magic(path: PathBuf) -> Option<String> {
    spawn_blocking(move || {
        infer::get_from_path(&path)
            .ok()
            .flatten()
            .map(|kind| kind.mime_type().to_owned())
    })
    .await
    .ok()
    .flatten()
}

async fn probe_dimensions(path: PathBuf) -> Option<(u32, u32)> {
    spawn_blocking(move || {
        ImageReader::open(&path)
            .ok()?
            .with_guessed_format()
            .ok()?
            .into_dimensions()
            .ok()
    })
    .await
    .ok()
    .flatten()
}

fn launcher_safe_extension(data: &[u8]) -> Option<&'static str> {
    launcher_safe_image_extension(data)
        .or_else(|| container::launcher_safe_video_extension(data))
        .or_else(|| container::launcher_safe_audio_extension(data))
}

fn launcher_safe_image_extension(data: &[u8]) -> Option<&'static str> {
    let format = image::guess_format(data).ok()?;
    let extension = launcher_safe_extension_for(format)?;
    if format.reading_enabled() && !header_parses(data, format) {
        return None;
    }
    Some(extension)
}

fn launcher_safe_extension_for(format: ImageFormat) -> Option<&'static str> {
    LAUNCHER_SAFE_IMAGE_FORMATS
        .iter()
        .find(|(launcher_safe, _)| *launcher_safe == format)
        .map(|(_, extension)| *extension)
}

fn header_parses(data: &[u8], format: ImageFormat) -> bool {
    ImageReader::with_format(Cursor::new(data), format)
        .into_dimensions()
        .is_ok()
}

fn open_root() -> Option<PathBuf> {
    let root = adopt_stable_root().or_else(unpredictable_root)?;
    sweep_stale(&root);
    let temp_root = env::temp_dir().join(MEDIA_DIR);
    if temp_root != root && private_fs::is_private_dir(&temp_root) {
        sweep_stale(&temp_root);
    }
    Some(root)
}

fn adopt_stable_root() -> Option<PathBuf> {
    let stable_root = per_user_dir().unwrap_or_else(env::temp_dir).join(MEDIA_DIR);
    match claim_dir(&stable_root) {
        Claim::Created | Claim::Adopted => Some(stable_root),
        Claim::Rejected => {
            tracing::warn!(
                path = %stable_root.display(),
                "media directory is not private to this user, falling back to an unpredictable one"
            );
            None
        }
    }
}

fn unpredictable_root() -> Option<PathBuf> {
    let parent = env::temp_dir();
    (0..ROOT_ATTEMPTS)
        .map(|_| parent.join(format!("{MEDIA_DIR}-{}", random_hex(ROOT_TOKEN_BYTES))))
        .find(|candidate| matches!(claim_dir(candidate), Claim::Created))
}

fn open_session_dir(root: &Path) -> Option<PathBuf> {
    let session_dir = root.join(format!(
        "session-{}-{}",
        process::id(),
        random_hex(SESSION_TOKEN_BYTES)
    ));
    matches!(claim_dir(&session_dir), Claim::Created).then_some(session_dir)
}

enum Claim {
    Created,
    Adopted,
    Rejected,
}

fn claim_dir(dir: &Path) -> Claim {
    match private_fs::create_dir_exclusive_blocking(dir) {
        Ok(()) => Claim::Created,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            if private_fs::is_private_dir(dir) {
                Claim::Adopted
            } else {
                Claim::Rejected
            }
        }
        Err(e) => {
            tracing::debug!("failed to create media directory {}: {e}", dir.display());
            Claim::Rejected
        }
    }
}

#[cfg(unix)]
fn per_user_dir() -> Option<PathBuf> {
    env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute() && private_fs::is_private_dir(dir))
}

#[cfg(not(unix))]
fn per_user_dir() -> Option<PathBuf> {
    None
}

fn sweep_stale(base_dir: &Path) {
    let Ok(entries) = fs::read_dir(base_dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(metadata) = own_stale_entry(&path, now) else {
            continue;
        };
        remove_entry(&path, metadata.is_dir());
    }
}

fn own_stale_entry(path: &Path, now: SystemTime) -> Option<fs::Metadata> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !private_fs::is_owned_by_current_user(&metadata) {
        return None;
    }
    let age = now.duration_since(metadata.modified().ok()?).ok()?;
    (age > MEDIA_RETENTION).then_some(metadata)
}

fn remove_entry(path: &Path, is_dir: bool) {
    let result = if is_dir {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    if let Err(e) = result {
        tracing::debug!("failed to remove stale media entry {}: {e}", path.display());
    }
}
