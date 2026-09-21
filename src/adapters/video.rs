use std::path::Path;
use std::time::Duration;

use crate::domain::media::Waveform;

pub struct VideoProbe {
    pub width: u32,
    pub height: u32,
    pub duration: Option<Duration>,
}

#[cfg(not(feature = "video"))]
pub fn probe(_path: &Path) -> Option<VideoProbe> {
    None
}

#[cfg(not(feature = "video"))]
pub fn poster_jpeg(_path: &Path, _max_edge: u32, _quality: u8) -> Option<Vec<u8>> {
    None
}

pub struct AudioProbe {
    pub duration: Option<Duration>,
    pub waveform: Option<Waveform>,
}

#[cfg(not(feature = "video"))]
pub fn probe_audio(_path: &Path) -> Option<AudioProbe> {
    None
}

#[cfg(feature = "video")]
pub mod audio_player;
#[cfg(feature = "video")]
mod decoder;
#[cfg(feature = "video")]
mod feed;
#[cfg(feature = "video")]
mod orientation;
#[cfg(feature = "video")]
mod output;
#[cfg(feature = "video")]
pub mod playback;
#[cfg(feature = "video")]
pub mod player;
#[cfg(feature = "video")]
mod waveform;

#[cfg(feature = "video")]
fn ffmpeg_ready() -> bool {
    use std::sync::OnceLock;
    static READY: OnceLock<bool> = OnceLock::new();
    *READY.get_or_init(|| match ffmpeg_next::init() {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("ffmpeg failed to initialise, video is unavailable: {e}");
            false
        }
    })
}

#[cfg(feature = "video")]
fn scaled_extent(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= max_edge || longest == 0 {
        return (width.max(1), height.max(1));
    }
    let scale = f64::from(max_edge) / f64::from(longest);
    let scale_edge = |edge: u32| {
        let scaled = (f64::from(edge) * scale).round();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let scaled = scaled as u32;
        scaled.max(1)
    };
    (scale_edge(width), scale_edge(height))
}

#[cfg(feature = "video")]
mod backend {
    use std::path::Path;
    use std::time::Duration;

    use ffmpeg_next::format::Pixel;
    use ffmpeg_next::software::scaling;
    use ffmpeg_next::util::frame::Video as VideoFrame;
    use ffmpeg_next::{codec, format, media};
    use image::RgbImage;
    use image::codecs::jpeg::JpegEncoder;
    use image::metadata::Orientation;

    use super::{VideoProbe, orientation};

    fn duration_of(input: &format::context::Input) -> Option<Duration> {
        let micros = input.duration();
        (micros > 0).then(|| Duration::from_micros(micros.unsigned_abs()))
    }

    pub(super) fn probe(path: &Path) -> Option<VideoProbe> {
        if !super::ffmpeg_ready() {
            return None;
        }
        let input = format::input(path).ok()?;
        let duration = duration_of(&input);
        let stream = input.streams().best(media::Type::Video)?;
        let decoder = codec::context::Context::from_parameters(stream.parameters())
            .ok()?
            .decoder()
            .video()
            .ok()?;
        let (width, height) = orientation::displayed_extent(
            decoder.width(),
            decoder.height(),
            orientation::of_stream(&stream),
        );
        Some(VideoProbe {
            width,
            height,
            duration,
        })
    }

    struct FirstFrame {
        frame: VideoFrame,
        width: u32,
        height: u32,
        format: Pixel,
        orientation: Orientation,
    }

    fn first_frame(path: &Path) -> Option<FirstFrame> {
        let mut input = format::input(path).ok()?;
        let (index, parameters, orientation) = {
            let stream = input.streams().best(media::Type::Video)?;
            (
                stream.index(),
                stream.parameters(),
                orientation::of_stream(&stream),
            )
        };
        let mut decoder = codec::context::Context::from_parameters(parameters)
            .ok()?
            .decoder()
            .video()
            .ok()?;
        let frame = decode_first(&mut input, index, &mut decoder)?;
        Some(FirstFrame {
            frame,
            width: decoder.width(),
            height: decoder.height(),
            format: decoder.format(),
            orientation,
        })
    }

    fn decode_first(
        input: &mut format::context::Input,
        index: usize,
        decoder: &mut codec::decoder::Video,
    ) -> Option<VideoFrame> {
        let mut frame = VideoFrame::empty();
        for (stream, packet) in input.packets() {
            if stream.index() != index {
                continue;
            }
            if decoder.send_packet(&packet).is_err() {
                continue;
            }
            if decoder.receive_frame(&mut frame).is_ok() {
                return Some(frame);
            }
        }
        decoder.send_eof().ok()?;
        decoder.receive_frame(&mut frame).ok()?;
        Some(frame)
    }

    pub(super) fn poster_jpeg(path: &Path, max_edge: u32, quality: u8) -> Option<Vec<u8>> {
        if !super::ffmpeg_ready() {
            return None;
        }
        let first = first_frame(path)?;
        let (target_width, target_height) =
            super::scaled_extent(first.width, first.height, max_edge);
        let mut scaler = scaling::Context::get(
            first.format,
            first.width,
            first.height,
            Pixel::RGB24,
            target_width,
            target_height,
            scaling::Flags::BILINEAR,
        )
        .ok()?;
        let mut rgb = VideoFrame::empty();
        scaler.run(&first.frame, &mut rgb).ok()?;

        let stride = rgb.stride(0);
        let row_bytes = target_width as usize * 3;
        let mut packed = Vec::with_capacity(row_bytes * target_height as usize);
        for row in rgb.data(0).chunks_exact(stride).take(target_height as usize) {
            packed.extend_from_slice(row.get(..row_bytes)?);
        }

        let as_stored = RgbImage::from_raw(target_width, target_height, packed)?;
        let poster = orientation::upright(as_stored, first.orientation);
        let mut bytes = Vec::new();
        let encoder = JpegEncoder::new_with_quality(&mut bytes, quality);
        poster.write_with_encoder(encoder).ok()?;
        Some(bytes)
    }
}

#[cfg(feature = "video")]
pub fn probe(path: &Path) -> Option<VideoProbe> {
    backend::probe(path)
}

#[cfg(feature = "video")]
pub fn poster_jpeg(path: &Path, max_edge: u32, quality: u8) -> Option<Vec<u8>> {
    backend::poster_jpeg(path, max_edge, quality)
}

#[cfg(feature = "video")]
pub fn probe_audio(path: &Path) -> Option<AudioProbe> {
    waveform::probe_audio(path)
}
