use std::path::PathBuf;

use arboard::Clipboard;
use url::Url;

use crate::domain::media::{PastedImage, PastedMedia};

pub fn pasted_media() -> Option<PastedMedia> {
    let mut clipboard = Clipboard::new()
        .inspect_err(|e| tracing::debug!("the clipboard is unavailable: {e}"))
        .ok()?;
    if let Some(file) = copied_file(&mut clipboard) {
        return Some(PastedMedia::File(file));
    }
    if clipboard
        .get_text()
        .is_ok_and(|text| says_more_than_a_link(&text))
    {
        return None;
    }
    copied_image(&mut clipboard).map(PastedMedia::Image)
}

fn copied_file(clipboard: &mut Clipboard) -> Option<PathBuf> {
    clipboard
        .get()
        .file_list()
        .ok()?
        .into_iter()
        .map(without_uri_list_carriage_return)
        .find(|path| path.is_file())
}

fn without_uri_list_carriage_return(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|text| text.strip_suffix('\r')) {
        Some(stripped) => PathBuf::from(stripped),
        None => path,
    }
}

fn copied_image(clipboard: &mut Clipboard) -> Option<PastedImage> {
    let image = clipboard.get_image().ok()?;
    PastedImage::new(
        u32::try_from(image.width).ok()?,
        u32::try_from(image.height).ok()?,
        image.bytes.into_owned(),
    )
}

fn says_more_than_a_link(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty() && (text.contains(char::is_whitespace) || Url::parse(text).is_err())
}
