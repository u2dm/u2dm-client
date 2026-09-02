use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use matrix_sdk::attachment::{
    AttachmentInfo, BaseFileInfo, BaseImageInfo, BaseVideoInfo, Thumbnail,
};
use matrix_sdk::ruma::UInt;
use mime::Mime;

use crate::domain::media::PickedAttachment;

const THUMBNAIL_MAX_EDGE: u32 = 800;
const THUMBNAIL_JPEG_QUALITY: u8 = 80;
const THUMBNAIL_DECODE_MAX_EDGE: u32 = 16_384;
const THUMBNAIL_DECODE_MAX_ALLOC: u64 =
    4 * THUMBNAIL_DECODE_MAX_EDGE as u64 * THUMBNAIL_DECODE_MAX_EDGE as u64;

const RECODING_LOSES_MEANING: &[&str] = &["image/gif", "image/webp", "image/svg+xml"];

pub(super) fn content_type(picked: &PickedAttachment, as_document: bool) -> Mime {
    if as_document {
        return mime::APPLICATION_OCTET_STREAM;
    }
    picked
        .mimetype
        .parse()
        .unwrap_or(mime::APPLICATION_OCTET_STREAM)
}

pub(super) fn attachment_info(picked: &PickedAttachment, content_type: &Mime) -> AttachmentInfo {
    let size = UInt::new(picked.size);
    let (width, height) = picked.dimensions.map_or((None, None), |(w, h)| {
        (UInt::new(w.into()), UInt::new(h.into()))
    });

    match content_type.type_() {
        mime::IMAGE => AttachmentInfo::Image(BaseImageInfo {
            width,
            height,
            size,
            blurhash: None,
            is_animated: None,
        }),
        mime::VIDEO => AttachmentInfo::Video(BaseVideoInfo {
            duration: None,
            width,
            height,
            size,
            blurhash: None,
        }),
        _ => AttachmentInfo::File(BaseFileInfo { size }),
    }
}

pub(super) fn wants_thumbnail(picked: &PickedAttachment, content_type: &Mime) -> bool {
    if content_type.type_() != mime::IMAGE {
        return false;
    }
    if RECODING_LOSES_MEANING
        .iter()
        .any(|kept| kept.eq_ignore_ascii_case(picked.mimetype.as_str()))
    {
        return false;
    }
    picked
        .dimensions
        .is_none_or(|(w, h)| w > THUMBNAIL_MAX_EDGE || h > THUMBNAIL_MAX_EDGE)
}

pub(super) fn make_thumbnail(data: &[u8]) -> Option<Thumbnail> {
    let image = decode(data)?;
    let scaled = image.thumbnail(THUMBNAIL_MAX_EDGE, THUMBNAIL_MAX_EDGE);
    let (bytes, content_type) = encode(&scaled)?;

    Some(Thumbnail {
        width: UInt::new(scaled.width().into())?,
        height: UInt::new(scaled.height().into())?,
        size: UInt::new(bytes.len().try_into().ok()?)?,
        data: bytes,
        content_type,
    })
}

fn decode(data: &[u8]) -> Option<DynamicImage> {
    let mut limits = Limits::no_limits();
    limits.max_image_width = Some(THUMBNAIL_DECODE_MAX_EDGE);
    limits.max_image_height = Some(THUMBNAIL_DECODE_MAX_EDGE);
    limits.max_alloc = Some(THUMBNAIL_DECODE_MAX_ALLOC);

    let mut reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .ok()?;
    reader.limits(limits);
    reader.decode().ok()
}

fn encode(image: &DynamicImage) -> Option<(Vec<u8>, Mime)> {
    let mut bytes = Vec::new();
    if image.color().has_alpha() {
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .ok()?;
        return Some((bytes, mime::IMAGE_PNG));
    }
    let encoder = JpegEncoder::new_with_quality(&mut bytes, THUMBNAIL_JPEG_QUALITY);
    image.to_rgb8().write_with_encoder(encoder).ok()?;
    Some((bytes, mime::IMAGE_JPEG))
}
