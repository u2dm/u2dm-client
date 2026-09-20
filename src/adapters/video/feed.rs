use std::time::Duration;

use ffmpeg_next::{Packet, format};

use super::decoder::{AudioDecoder, PcmTarget};
use super::output::AudioOutput;

pub struct AudioFeed {
    decoder: AudioDecoder,
    output: AudioOutput,
    trim_until: Option<Duration>,
    horizon: Duration,
}

impl AudioFeed {
    pub fn open(input: &format::context::Input, output: AudioOutput) -> Option<Self> {
        let target = PcmTarget {
            rate: output.sample_rate(),
            channels: output.channels(),
        };
        let decoder = AudioDecoder::open(input, target)?;
        Some(Self {
            decoder,
            output,
            trim_until: None,
            horizon: Duration::ZERO,
        })
    }

    pub fn output(&self) -> &AudioOutput {
        &self.output
    }

    pub fn stream_index(&self) -> usize {
        self.decoder.stream_index()
    }

    pub fn horizon(&self) -> Duration {
        self.horizon
    }

    pub fn feed(&mut self, packet: &Packet) {
        let Self {
            decoder,
            output,
            trim_until,
            horizon,
        } = self;
        decoder.feed(packet, &mut |samples, start| {
            *horizon = horizon_after(output, *horizon, samples, start);
            push_trimmed(output, trim_until, samples, start);
        });
    }

    pub fn finish(&mut self) {
        let Self {
            decoder,
            output,
            trim_until,
            horizon,
        } = self;
        decoder.finish(&mut |samples, start| {
            *horizon = horizon_after(output, *horizon, samples, start);
            push_trimmed(output, trim_until, samples, start);
        });
    }

    pub fn rebase(&mut self, position: Duration) {
        self.decoder.reset();
        self.output.rebase(position);
        self.trim_until = Some(position);
        self.horizon = position;
    }
}

fn horizon_after(
    output: &AudioOutput,
    horizon: Duration,
    samples: &[f32],
    start: Option<Duration>,
) -> Duration {
    let lanes = usize::from(output.channels().max(1));
    let frames = u64::try_from(samples.len() / lanes).unwrap_or(u64::MAX);
    let span = Duration::from_micros(
        frames.saturating_mul(1_000_000) / u64::from(output.sample_rate().max(1)),
    );
    horizon.max(start.unwrap_or(horizon).saturating_add(span))
}

fn push_trimmed(
    output: &AudioOutput,
    trim_until: &mut Option<Duration>,
    samples: &[f32],
    start: Option<Duration>,
) {
    let (Some(until), Some(start)) = (*trim_until, start) else {
        *trim_until = None;
        output.push(samples);
        return;
    };
    let Some(ahead) = until.checked_sub(start) else {
        *trim_until = None;
        output.push(samples);
        return;
    };
    let frames = ahead
        .as_micros()
        .saturating_mul(u128::from(output.sample_rate()))
        / 1_000_000;
    let skip = usize::try_from(frames)
        .unwrap_or(usize::MAX)
        .saturating_mul(usize::from(output.channels()));
    if let Some(rest) = samples.get(skip..).filter(|rest| !rest.is_empty()) {
        *trim_until = None;
        output.push(rest);
    }
}
