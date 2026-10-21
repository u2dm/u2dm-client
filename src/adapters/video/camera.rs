use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use ffmpeg_next::codec::decoder::Video as VideoDecoder;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::format::context::{Context, Input};
use ffmpeg_next::software::scaling::{Context as Scaler, Flags};
use ffmpeg_next::util::frame::Video as VideoFrame;
use ffmpeg_next::{Dictionary, codec, device, format, media};

const SYSFS_VIDEO: &str = "/sys/class/video4linux";
const DEVICE_DIR: &str = "/dev";
const CAPTURE_NODE: &str = "0";
const V4L2_NAMES: [&str; 2] = ["video4linux2", "v4l2"];
const LARGEST: (u32, u32) = (1280, 720);
const ASKED_FORMATS: [Asked; 4] = [
    Asked::Format("mjpeg", "1280x720"),
    Asked::Format("yuyv422", "1280x720"),
    Asked::Format("yuyv422", "640x480"),
    Asked::Anything,
];
const ASKED_RATE: &str = "30";

#[derive(Clone, Copy)]
enum Asked {
    Format(&'static str, &'static str),
    Anything,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraDevice {
    pub path: PathBuf,
    pub name: String,
}

pub fn cameras() -> Vec<CameraDevice> {
    let Ok(entries) = fs::read_dir(SYSFS_VIDEO) else {
        return Vec::new();
    };
    let mut found: Vec<CameraDevice> = entries
        .flatten()
        .filter(|entry| read_trimmed(&entry.path().join("index")).as_deref() == Some(CAPTURE_NODE))
        .map(|entry| CameraDevice {
            path: Path::new(DEVICE_DIR).join(entry.file_name()),
            name: read_trimmed(&entry.path().join("name")).unwrap_or_default(),
        })
        .collect();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
}

pub struct Planes<'a> {
    pub width: u32,
    pub height: u32,
    pub y: (&'a [u8], usize),
    pub u: (&'a [u8], usize),
    pub v: (&'a [u8], usize),
    pub timestamp_us: i64,
}

pub struct Capture {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    pub fn start(
        device: &CameraDevice,
        on_frame: impl FnMut(&Planes<'_>) + Send + 'static,
        on_lost: impl FnOnce(String) + Send + 'static,
    ) -> Result<Self, String> {
        if !super::ffmpeg_ready() {
            return Err("FFmpeg did not start".into());
        }
        let running = Arc::new(AtomicBool::new(true));
        let (opened_tx, opened_rx) = mpsc::channel();
        let flag = Arc::clone(&running);
        let path = device.path.clone();
        let thread = thread::Builder::new()
            .name("u2dm-camera".into())
            .spawn(move || match Source::open(&path) {
                Ok(source) => {
                    drop(opened_tx.send(Ok(())));
                    source.run(&flag, on_frame, on_lost);
                }
                Err(why) => drop(opened_tx.send(Err(why))),
            })
            .map_err(|e| format!("the camera thread did not start: {e}"))?;
        let opened = opened_rx
            .recv()
            .unwrap_or_else(|_| Err("the camera thread ended before opening".into()));
        let mut capture = Self {
            running,
            thread: Some(thread),
        };
        match opened {
            Ok(()) => Ok(capture),
            Err(why) => {
                capture.stop();
                Err(why)
            }
        }
    }

    pub fn is_running(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::warn!("the camera thread panicked");
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Source {
    input: Input,
    stream: usize,
    decoder: VideoDecoder,
    scaler: Option<Scaler>,
}

impl Source {
    fn open(path: &Path) -> Result<Self, String> {
        let v4l2 = device::input::video()
            .find(|found| {
                found
                    .name()
                    .split(',')
                    .any(|name| V4L2_NAMES.contains(&name))
            })
            .ok_or_else(|| "this FFmpeg has no video4linux2 input".to_owned())?;
        let mut refused = Vec::new();
        for asked in ASKED_FORMATS {
            let started = Instant::now();
            match format::open_with(path, &v4l2, options(asked)) {
                Ok(Context::Input(input)) => {
                    tracing::info!(
                        device = %path.display(),
                        took_ms = started.elapsed().as_millis(),
                        asked = describe(asked),
                        "opened the camera"
                    );
                    return Self::decoding(input);
                }
                Ok(Context::Output(_)) => refused.push("an output context".to_owned()),
                Err(e) => refused.push(format!("{}: {e}", describe(asked))),
            }
        }
        Err(format!("the camera did not open ({})", refused.join("; ")))
    }

    fn decoding(input: Input) -> Result<Self, String> {
        let stream = input
            .streams()
            .best(media::Type::Video)
            .ok_or_else(|| "the camera offers no video".to_owned())?;
        let index = stream.index();
        let decoder = codec::context::Context::from_parameters(stream.parameters())
            .and_then(|context| context.decoder().video())
            .map_err(|e| format!("the camera's video does not decode: {e}"))?;
        tracing::info!(
            codec = ?decoder.id(),
            width = decoder.width(),
            height = decoder.height(),
            format = ?decoder.format(),
            "the camera streams"
        );
        Ok(Self {
            input,
            stream: index,
            decoder,
            scaler: None,
        })
    }

    fn run(
        mut self,
        running: &AtomicBool,
        on_frame: impl FnMut(&Planes<'_>),
        on_lost: impl FnOnce(String),
    ) {
        self.pump(running, on_frame);
        self.release_buffers_then_close();
        if running.load(Ordering::Acquire) {
            on_lost("the camera stopped sending pictures".into());
        }
    }

    fn pump(&mut self, running: &AtomicBool, mut on_frame: impl FnMut(&Planes<'_>)) {
        let started = Instant::now();
        let mut decoded = VideoFrame::empty();
        let mut i420 = VideoFrame::empty();
        for (stream, packet) in self.input.packets() {
            if !running.load(Ordering::Acquire) {
                break;
            }
            if stream.index() != self.stream || self.decoder.send_packet(&packet).is_err() {
                continue;
            }
            while self.decoder.receive_frame(&mut decoded).is_ok() {
                if let Some(scaler) = scaler_for(&mut self.scaler, &decoded)
                    && scaler.run(&decoded, &mut i420).is_ok()
                {
                    on_frame(&planes(&i420, started));
                }
            }
        }
    }

    fn release_buffers_then_close(self) {
        let Self {
            input,
            decoder,
            scaler,
            ..
        } = self;
        drop(scaler);
        drop(decoder);
        drop(input);
    }
}

fn options(asked: Asked) -> Dictionary<'static> {
    let mut options = Dictionary::new();
    if let Asked::Format(input_format, size) = asked {
        options.set("input_format", input_format);
        options.set("video_size", size);
        options.set("framerate", ASKED_RATE);
    }
    options
}

const fn describe(asked: Asked) -> &'static str {
    match asked {
        Asked::Format(input_format, _) => input_format,
        Asked::Anything => "its default format",
    }
}

fn scaler_for<'a>(slot: &'a mut Option<Scaler>, frame: &VideoFrame) -> Option<&'a mut Scaler> {
    if slot.is_none() {
        let (width, height) = fitted(frame.width(), frame.height());
        match Scaler::get(
            frame.format(),
            frame.width(),
            frame.height(),
            Pixel::YUV420P,
            width,
            height,
            Flags::BILINEAR,
        ) {
            Ok(scaler) => *slot = Some(scaler),
            Err(e) => {
                tracing::warn!("the camera's pictures cannot be converted: {e}");
                return None;
            }
        }
    }
    slot.as_mut()
}

fn fitted(width: u32, height: u32) -> (u32, u32) {
    let (largest_width, largest_height) = LARGEST;
    if width <= largest_width && height <= largest_height {
        return (width & !1, height & !1);
    }
    let scale = (f64::from(largest_width) / f64::from(width))
        .min(f64::from(largest_height) / f64::from(height));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let side = |edge: u32| ((f64::from(edge) * scale) as u32) & !1;
    (side(width), side(height))
}

fn planes(frame: &VideoFrame, started: Instant) -> Planes<'_> {
    Planes {
        width: frame.width(),
        height: frame.height(),
        y: (frame.data(0), frame.stride(0)),
        u: (frame.data(1), frame.stride(1)),
        v: (frame.data(2), frame.stride(2)),
        timestamp_us: i64::try_from(started.elapsed().as_micros()).unwrap_or(i64::MAX),
    }
}
