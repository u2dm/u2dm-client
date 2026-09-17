use std::path::Path;
use std::time::Duration;

use ffmpeg_next::{format, media};

use super::AudioProbe;
use super::decoder::{AudioDecoder, PcmTarget};
use super::playback::stream_duration;
use crate::domain::media::Waveform;

const ENVELOPE_RATE: u32 = 8_000;
const ENVELOPE_WINDOW: usize = 80;
const WAVEFORM_SAMPLES: usize = 100;
const WAVEFORM_DECODE_LIMIT: Duration = Duration::from_mins(10);

pub fn probe_audio(path: &Path) -> Option<AudioProbe> {
    if !super::ffmpeg_ready() {
        return None;
    }
    let mut input = format::input(path).ok()?;
    let stream_index = input.streams().best(media::Type::Audio)?.index();
    let duration = stream_duration(&input);
    let target = PcmTarget {
        rate: ENVELOPE_RATE,
        channels: 1,
    };
    let waveform = AudioDecoder::open(&input, target)
        .and_then(|decoder| envelope_of(&mut input, stream_index, decoder));
    Some(AudioProbe { duration, waveform })
}

fn envelope_of(
    input: &mut format::context::Input,
    stream_index: usize,
    mut decoder: AudioDecoder,
) -> Option<Waveform> {
    let mut envelope = Envelope::default();
    for (stream, packet) in input.packets() {
        if stream.index() != stream_index {
            continue;
        }
        decoder.feed(&packet, &mut |samples, _| envelope.absorb(samples));
        if envelope.covers(WAVEFORM_DECODE_LIMIT) {
            break;
        }
    }
    decoder.finish(&mut |samples, _| envelope.absorb(samples));
    envelope.into_waveform()
}

#[derive(Default)]
struct Envelope {
    peaks: Vec<f32>,
    window_peak: f32,
    window_fill: usize,
    total_samples: u64,
}

impl Envelope {
    fn absorb(&mut self, samples: &[f32]) {
        for sample in samples {
            self.window_peak = self.window_peak.max(sample.abs());
            self.window_fill += 1;
            if self.window_fill == ENVELOPE_WINDOW {
                self.close_window();
            }
        }
        self.total_samples = self.total_samples.saturating_add(samples.len() as u64);
    }

    fn close_window(&mut self) {
        self.peaks.push(self.window_peak);
        self.window_peak = 0.0;
        self.window_fill = 0;
    }

    fn covers(&self, span: Duration) -> bool {
        self.total_samples / u64::from(ENVELOPE_RATE) >= span.as_secs()
    }

    fn into_waveform(mut self) -> Option<Waveform> {
        if self.window_fill > 0 {
            self.close_window();
        }
        let bucket = self.peaks.len().div_ceil(WAVEFORM_SAMPLES).max(1);
        let buckets: Vec<f32> = self
            .peaks
            .chunks(bucket)
            .map(|chunk| chunk.iter().copied().fold(0.0, f32::max))
            .collect();
        Waveform::from_levels(&buckets)
    }
}
