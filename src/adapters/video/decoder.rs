use std::time::Duration;

use ffmpeg_next::util::format::sample::{Sample, Type as SampleType};
use ffmpeg_next::util::frame::Audio as AudioFrame;
use ffmpeg_next::{ChannelLayout, Packet, Rational, codec, format, media, software};

const RESAMPLE_HEADROOM: usize = 1024;

pub type Emit<'a> = &'a mut dyn FnMut(&[f32], Option<Duration>);

#[derive(Clone, Copy)]
pub struct PcmTarget {
    pub rate: u32,
    pub channels: u16,
}

impl PcmTarget {
    fn layout(self) -> ChannelLayout {
        match self.channels {
            1 => ChannelLayout::MONO,
            _ => ChannelLayout::STEREO,
        }
    }
}

pub struct AudioDecoder {
    decoder: codec::decoder::Audio,
    resampler: software::resampling::Context,
    target: PcmTarget,
    stream_index: usize,
    time_base: Rational,
}

impl AudioDecoder {
    pub fn open(input: &format::context::Input, target: PcmTarget) -> Option<Self> {
        let stream = input.streams().best(media::Type::Audio)?;
        let stream_index = stream.index();
        let time_base = stream.time_base();
        let decoder = codec::context::Context::from_parameters(stream.parameters())
            .ok()?
            .decoder()
            .audio()
            .ok()?;
        let resampler = resampler_for(&decoder, target)?;
        Some(Self {
            decoder,
            resampler,
            target,
            stream_index,
            time_base,
        })
    }

    pub fn stream_index(&self) -> usize {
        self.stream_index
    }

    pub fn feed(&mut self, packet: &Packet, emit: Emit<'_>) {
        if self.decoder.send_packet(packet).is_err() {
            return;
        }
        self.drain_decoder(emit);
    }

    pub fn finish(&mut self, emit: Emit<'_>) {
        if self.decoder.send_eof().is_ok() {
            self.drain_decoder(emit);
        }
        let mut flushed = self.output_frame(RESAMPLE_HEADROOM);
        if self.resampler.flush(&mut flushed).is_ok() {
            self.emit_resampled(&flushed, None, emit);
        }
    }

    pub fn reset(&mut self) {
        self.decoder.flush();
        if let Some(resampler) = resampler_for(&self.decoder, self.target) {
            self.resampler = resampler;
        }
    }

    fn drain_decoder(&mut self, emit: Emit<'_>) {
        let mut decoded = AudioFrame::empty();
        while self.decoder.receive_frame(&mut decoded).is_ok() {
            decoded.set_channel_layout(named_layout(decoded.channel_layout()));
            let start = decoded.pts().or_else(|| decoded.timestamp());
            let mut resampled = self.output_frame(self.resampled_capacity(decoded.samples()));
            if self.resampler.run(&decoded, &mut resampled).is_err() {
                continue;
            }
            let start = start.map(|ticks| ticks_to_duration(ticks, self.time_base));
            self.emit_resampled(&resampled, start, emit);
        }
    }

    fn output_frame(&self, capacity: usize) -> AudioFrame {
        AudioFrame::new(
            Sample::F32(SampleType::Packed),
            capacity,
            self.target.layout(),
        )
    }

    fn emit_resampled(&self, resampled: &AudioFrame, start: Option<Duration>, emit: Emit<'_>) {
        let lanes = usize::from(self.target.channels);
        let wanted = resampled.samples() * lanes * size_of::<f32>();
        let Some(bytes) = resampled.data(0).get(..wanted) else {
            return;
        };
        if bytes.is_empty() {
            return;
        }
        let samples: Vec<f32> = bytes
            .chunks_exact(size_of::<f32>())
            .map(|chunk| <[u8; 4]>::try_from(chunk).map_or(0.0, f32::from_ne_bytes))
            .collect();
        emit(&samples, start);
    }

    fn resampled_capacity(&self, samples: usize) -> usize {
        let source = u64::from(self.decoder.rate().max(1));
        let target = u64::from(self.target.rate);
        let scaled = (samples as u64).saturating_mul(target) / source;
        usize::try_from(scaled).unwrap_or(samples) + RESAMPLE_HEADROOM
    }
}

pub fn ticks_to_duration(ticks: i64, time_base: Rational) -> Duration {
    let numerator = i128::from(time_base.numerator());
    let denominator = i128::from(time_base.denominator());
    if denominator == 0 {
        return Duration::ZERO;
    }
    let micros = i128::from(ticks.max(0))
        .saturating_mul(numerator)
        .saturating_mul(1_000_000)
        / denominator;
    Duration::from_micros(u64::try_from(micros).unwrap_or(0))
}

fn resampler_for(
    decoder: &codec::decoder::Audio,
    target: PcmTarget,
) -> Option<software::resampling::Context> {
    software::resampling::Context::get(
        decoder.format(),
        named_layout(decoder.channel_layout()),
        decoder.rate(),
        Sample::F32(SampleType::Packed),
        target.layout(),
        target.rate,
    )
    .ok()
}

fn named_layout(layout: ChannelLayout) -> ChannelLayout {
    if layout.is_empty() {
        ChannelLayout::default(layout.channels())
    } else {
        layout
    }
}
