use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleFormat, StreamConfig, SupportedBufferSize, SupportedStreamConfig};

const BUFFERED_SECONDS: f64 = 1.0;
const DEVICE_PERIODS_PER_SECOND: u32 = 40;
const NULL_DRIVER: &str = "null";

#[derive(Default)]
struct Shared {
    samples: Mutex<VecDeque<f32>>,
    frames_played: AtomicU64,
    muted: AtomicBool,
}

pub struct AudioOutput {
    shared: Arc<Shared>,
    stream: Box<cpal::Stream>,
    sample_rate: u32,
    channels: u16,
    capacity: usize,
}

impl AudioOutput {
    pub fn open() -> Option<Self> {
        quietly(Self::probe)
    }

    fn probe() -> Option<Self> {
        let host = cpal::default_host();
        let preferred = host.default_output_device();
        let enumerated = host.output_devices().into_iter().flatten();
        for device in preferred.into_iter().chain(enumerated) {
            if discards_samples(&device) {
                continue;
            }
            if let Some(output) = Self::on(&device) {
                return Some(output);
            }
        }
        tracing::debug!("no audio output device would open, playing without sound");
        None
    }

    fn on(device: &cpal::Device) -> Option<Self> {
        let config = float_config(device)?;
        let sample_rate = config.sample_rate;
        let channels = config.channels;

        let shared = Arc::new(Shared::default());
        let sink = Arc::clone(&shared);
        let lanes = usize::from(channels);
        let stream = device
            .build_output_stream(
                config,
                move |output: &mut [f32], _| fill(&sink, output, lanes),
                |e| tracing::warn!("the audio stream failed: {e}"),
                None,
            )
            .ok()?;
        if stream.play().is_err() {
            return None;
        }

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let capacity = (f64::from(sample_rate) * BUFFERED_SECONDS) as usize * lanes;
        Some(Self {
            shared,
            stream: Box::new(stream),
            sample_rate,
            channels,
            capacity,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn position(&self) -> Duration {
        let frames = self.shared.frames_played.load(Ordering::Relaxed);
        Duration::from_micros(
            frames.saturating_mul(1_000_000) / u64::from(self.sample_rate.max(1)),
        )
    }

    pub fn is_full(&self) -> bool {
        self.shared
            .samples
            .lock()
            .is_ok_and(|queue| queue.len() >= self.capacity)
    }

    pub fn queued_frames(&self) -> usize {
        self.shared
            .samples
            .lock()
            .map_or(0, |queue| queue.len() / usize::from(self.channels.max(1)))
    }

    pub fn push(&self, samples: &[f32]) {
        if let Ok(mut queue) = self.shared.samples.lock() {
            queue.extend(samples);
        }
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }

    pub fn resume(&self) {
        if self.stream.play().is_err() {
            tracing::debug!("the audio stream would not resume");
        }
    }

    pub fn pause(&self) {
        if self.stream.pause().is_err() {
            tracing::debug!("the audio stream would not pause");
        }
    }

    pub fn rebase(&self, position: Duration) {
        if let Ok(mut queue) = self.shared.samples.lock() {
            queue.clear();
        }
        let frames = position
            .as_micros()
            .saturating_mul(u128::from(self.sample_rate))
            / 1_000_000;
        self.shared
            .frames_played
            .store(u64::try_from(frames).unwrap_or(0), Ordering::Relaxed);
    }
}

#[cfg(not(target_os = "linux"))]
fn quietly<T>(probe: impl FnOnce() -> T) -> T {
    probe()
}

#[cfg(target_os = "linux")]
fn quietly<T>(probe: impl FnOnce() -> T) -> T {
    let Ok(captured) = alsa::Output::local_error_handler() else {
        return probe();
    };
    let probed = probe();
    for line in format!("{}", captured.borrow())
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        tracing::debug!("alsa: {line}");
    }
    probed
}

fn float_config(device: &cpal::Device) -> Option<StreamConfig> {
    let preferred = device.default_output_config().ok()?;
    let float = if preferred.sample_format() == SampleFormat::F32 {
        preferred
    } else {
        let Some(converted) = float_variant(device, &preferred) else {
            tracing::debug!(
                "the audio device wants {:?}, which this build does not convert to",
                preferred.sample_format()
            );
            return None;
        };
        converted
    };
    Some(StreamConfig {
        buffer_size: short_period(float.sample_rate(), float.buffer_size()),
        ..float.config()
    })
}

fn float_variant(
    device: &cpal::Device,
    preferred: &SupportedStreamConfig,
) -> Option<SupportedStreamConfig> {
    device
        .supported_output_configs()
        .ok()?
        .filter(|range| {
            range.sample_format() == SampleFormat::F32 && range.channels() == preferred.channels()
        })
        .find_map(|range| range.try_with_sample_rate(preferred.sample_rate()))
}

fn short_period(sample_rate: u32, supported: &SupportedBufferSize) -> BufferSize {
    let SupportedBufferSize::Range { min, max } = *supported else {
        return BufferSize::Default;
    };
    BufferSize::Fixed((sample_rate / DEVICE_PERIODS_PER_SECOND).max(min).min(max))
}

fn discards_samples(device: &cpal::Device) -> bool {
    device
        .description()
        .is_ok_and(|description| description.driver() == Some(NULL_DRIVER))
}

fn fill(shared: &Arc<Shared>, output: &mut [f32], lanes: usize) {
    let muted = shared.muted.load(Ordering::Relaxed);
    let mut delivered: usize = 0;
    if let Ok(mut queue) = shared.samples.lock() {
        for slot in output.iter_mut() {
            match queue.pop_front() {
                Some(sample) => {
                    *slot = if muted { 0.0 } else { sample };
                    delivered += 1;
                }
                None => *slot = 0.0,
            }
        }
    } else {
        output.fill(0.0);
    }
    if let Some(frames) = delivered.checked_div(lanes) {
        shared
            .frames_played
            .fetch_add(frames as u64, Ordering::Relaxed);
    }
}
