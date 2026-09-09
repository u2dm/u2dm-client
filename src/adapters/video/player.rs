use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ffmpeg_next::format::Pixel;
use ffmpeg_next::software::scaling;
use ffmpeg_next::util::frame::{Audio as AudioFrame, Video as VideoFrame};
use ffmpeg_next::Packet;
use ffmpeg_next::util::format::sample::{Sample, Type as SampleType};
use ffmpeg_next::{ChannelLayout, Rational, codec, format, media, software};

use super::audio::AudioOutput;

const MAX_DIMENSION: u32 = 1280;
const LATE_FRAME_TOLERANCE: Duration = Duration::from_millis(120);
const COMMAND_POLL: Duration = Duration::from_millis(4);
const MAX_PENDING_FRAMES: usize = 4;
const RESAMPLE_HEADROOM: usize = 1024;

struct Pending {
    rgb: Vec<u8>,
    position: Duration,
}

pub enum PlayerEvent<'a> {
    Ready {
        duration: Option<Duration>,
    },
    Frame {
        rgb: &'a [u8],
        width: u32,
        height: u32,
        position: Duration,
    },
    Ended,
    Failed,
}

pub type Sink = Box<dyn Fn(PlayerEvent<'_>) + Send>;

#[derive(Clone, Copy)]
enum Command {
    Play,
    Pause,
    Seek(Duration),
    Muted(bool),
    Stop,
}

pub struct Playback {
    commands: Sender<Command>,
    worker: Option<JoinHandle<()>>,
}

impl Playback {
    pub fn start(path: &Path, sink: Sink) -> Self {
        let (commands, inbox) = mpsc::channel();
        let path = path.to_path_buf();
        let worker = thread::Builder::new()
            .name("u2dm-video".into())
            .spawn(move || run(&path, &inbox, sink.as_ref()))
            .ok();
        Self { commands, worker }
    }

    pub fn play(&self) {
        self.send(Command::Play);
    }

    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    pub fn seek(&self, position: Duration) {
        self.send(Command::Seek(position));
    }

    pub fn set_muted(&self, muted: bool) {
        self.send(Command::Muted(muted));
    }

    fn send(&self, command: Command) {
        if self.commands.send(command).is_err() {
            tracing::debug!("video playback thread is gone, dropping the command");
        }
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.send(Command::Stop);
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::warn!("the video playback thread panicked");
        }
    }
}

enum Clock {
    Wall {
        origin: Instant,
        paused_at: Option<Instant>,
    },
    Audio {
        position: Duration,
        paused: bool,
    },
}

impl Clock {
    fn wall() -> Self {
        Self::Wall {
            origin: Instant::now(),
            paused_at: Some(Instant::now()),
        }
    }

    fn audio() -> Self {
        Self::Audio {
            position: Duration::ZERO,
            paused: true,
        }
    }

    fn is_paused(&self) -> bool {
        match self {
            Self::Wall { paused_at, .. } => paused_at.is_some(),
            Self::Audio { paused, .. } => *paused,
        }
    }

    fn resume(&mut self) {
        match self {
            Self::Wall { origin, paused_at } => {
                if let Some(at) = paused_at.take() {
                    *origin += at.elapsed();
                }
            }
            Self::Audio { paused, .. } => *paused = false,
        }
    }

    fn pause(&mut self) {
        match self {
            Self::Wall { paused_at, .. } => {
                if paused_at.is_none() {
                    *paused_at = Some(Instant::now());
                }
            }
            Self::Audio { paused, .. } => *paused = true,
        }
    }

    fn rebase(&mut self, to: Duration) {
        match self {
            Self::Wall { origin, paused_at } => {
                let now = paused_at.unwrap_or_else(Instant::now);
                *origin = now.checked_sub(to).unwrap_or(now);
            }
            Self::Audio { position, .. } => *position = to,
        }
    }

    fn observe(&mut self, heard: Duration) {
        if let Self::Audio { position, .. } = self {
            *position = heard;
        }
    }

    fn elapsed(&self) -> Duration {
        match self {
            Self::Wall { origin, paused_at } => {
                paused_at.unwrap_or_else(Instant::now).saturating_duration_since(*origin)
            }
            Self::Audio { position, .. } => *position,
        }
    }

    fn due_in(&self, position: Duration) -> Duration {
        position.saturating_sub(self.elapsed())
    }

    fn is_late(&self, position: Duration) -> bool {
        self.elapsed() > position + LATE_FRAME_TOLERANCE
    }
}

fn stream_duration(input: &format::context::Input) -> Option<Duration> {
    let micros = input.duration();
    (micros > 0).then(|| Duration::from_micros(micros.unsigned_abs()))
}

fn run(path: &PathBuf, inbox: &Receiver<Command>, sink: &(dyn Fn(PlayerEvent<'_>) + Send)) {
    match Session::open(path) {
        Some(mut session) => session.drive(inbox, sink),
        None => sink(PlayerEvent::Failed),
    }
}

struct Audio {
    decoder: codec::decoder::Audio,
    resampler: software::resampling::Context,
    output: AudioOutput,
    stream_index: usize,
    layout: ChannelLayout,
}

impl Audio {
    fn open(input: &format::context::Input) -> Option<Self> {
        let stream = input.streams().best(media::Type::Audio)?;
        let stream_index = stream.index();
        let decoder = codec::context::Context::from_parameters(stream.parameters())
            .ok()?
            .decoder()
            .audio()
            .ok()?;
        let output = AudioOutput::open()?;
        let layout = match output.channels() {
            1 => ChannelLayout::MONO,
            _ => ChannelLayout::STEREO,
        };
        let resampler = software::resampling::Context::get(
            decoder.format(),
            decoder.channel_layout(),
            decoder.rate(),
            Sample::F32(SampleType::Packed),
            layout,
            output.sample_rate(),
        )
        .ok()?;
        Some(Self {
            decoder,
            resampler,
            output,
            stream_index,
            layout,
        })
    }

    fn resampled_capacity(&self, samples: usize) -> usize {
        let source = u64::from(self.decoder.rate().max(1));
        let target = u64::from(self.output.sample_rate());
        let scaled = (samples as u64).saturating_mul(target) / source;
        usize::try_from(scaled).unwrap_or(samples) + RESAMPLE_HEADROOM
    }

    fn feed(&mut self, packet: &Packet) {
        if self.decoder.send_packet(packet).is_err() {
            return;
        }
        let mut decoded = AudioFrame::empty();
        while self.decoder.receive_frame(&mut decoded).is_ok() {
            let mut resampled = AudioFrame::new(
                Sample::F32(SampleType::Packed),
                self.resampled_capacity(decoded.samples()),
                self.layout,
            );
            if self.resampler.run(&decoded, &mut resampled).is_err() {
                continue;
            }
            let lanes = usize::from(self.output.channels());
            let wanted = resampled.samples() * lanes * size_of::<f32>();
            let Some(bytes) = resampled.data(0).get(..wanted) else {
                continue;
            };
            let mut samples = Vec::with_capacity(resampled.samples() * lanes);
            for chunk in bytes.chunks_exact(size_of::<f32>()) {
                let value = <[u8; 4]>::try_from(chunk).map_or(0.0, f32::from_ne_bytes);
                samples.push(value);
            }
            self.output.push(&samples);
        }
    }
}

struct Session {
    input: format::context::Input,
    decoder: codec::decoder::Video,
    scaler: scaling::Context,
    stream_index: usize,
    time_base: Rational,
    duration: Option<Duration>,
    width: u32,
    height: u32,
    audio: Option<Audio>,
    pending: VecDeque<Pending>,
    drained: bool,
}

impl Session {
    fn open(path: &PathBuf) -> Option<Self> {
        if !super::ffmpeg_ready() {
            return None;
        }
        let input = format::input(path).ok()?;
        let duration = stream_duration(&input);
        let stream = input.streams().best(media::Type::Video)?;
        let stream_index = stream.index();
        let time_base = stream.time_base();
        let decoder = codec::context::Context::from_parameters(stream.parameters())
            .ok()?
            .decoder()
            .video()
            .ok()?;
        let (width, height) = super::scaled_extent(decoder.width(), decoder.height(), MAX_DIMENSION);
        let scaler = scaling::Context::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            Pixel::RGB24,
            width,
            height,
            scaling::Flags::BILINEAR,
        )
        .ok()?;
        let audio = Audio::open(&input);
        if let Some(audio) = audio.as_ref() {
            tracing::debug!(
                sample_rate = audio.output.sample_rate(),
                channels = audio.output.channels(),
                "video playback opened an audio device"
            );
        } else {
            tracing::debug!("video playback has no audio, pacing on the wall clock");
        }
        Some(Self {
            input,
            decoder,
            scaler,
            stream_index,
            time_base,
            duration,
            width,
            height,
            audio,
            pending: VecDeque::new(),
            drained: false,
        })
    }

    fn position_of(&self, frame: &VideoFrame) -> Duration {
        let ticks = i128::from(frame.pts().or_else(|| frame.timestamp()).unwrap_or(0).max(0));
        let numerator = i128::from(self.time_base.numerator());
        let denominator = i128::from(self.time_base.denominator());
        if denominator == 0 {
            return Duration::ZERO;
        }
        let micros = ticks.saturating_mul(numerator).saturating_mul(1_000_000) / denominator;
        Duration::from_micros(u64::try_from(micros).unwrap_or(0))
    }

    fn seek_to(&mut self, position: Duration) {
        let target = i64::try_from(position.as_micros()).unwrap_or(i64::MAX);
        if self.input.seek(target, ..target).is_err() {
            tracing::debug!("seek failed, staying where we are");
            return;
        }
        self.decoder.flush();
    }

    fn drive(&mut self, inbox: &Receiver<Command>, sink: &(dyn Fn(PlayerEvent<'_>) + Send)) {
        sink(PlayerEvent::Ready {
            duration: self.duration,
        });
        let mut clock = if self.audio.is_some() {
            Clock::audio()
        } else {
            Clock::wall()
        };
        loop {
            match self.pump(inbox, sink, &mut clock) {
                Flow::Continue => {}
                Flow::Stop => return,
                Flow::Ended => {
                    sink(PlayerEvent::Ended);
                    clock.pause();
                    if let Some(audio) = self.audio.as_ref() {
                        audio.output.pause();
                    }
                    if matches!(self.wait_for_command(inbox, &mut clock), Flow::Stop) {
                        return;
                    }
                }
            }
        }
    }

    fn wait_for_command(&mut self, inbox: &Receiver<Command>, clock: &mut Clock) -> Flow {
        loop {
            match inbox.recv() {
                Ok(Command::Stop) | Err(_) => return Flow::Stop,
                Ok(command) => {
                    self.apply(command, clock);
                    if !clock.is_paused() {
                        return Flow::Continue;
                    }
                }
            }
        }
    }

    fn apply(&mut self, command: Command, clock: &mut Clock) {
        match command {
            Command::Play => {
                clock.resume();
                if let Some(audio) = self.audio.as_ref() {
                    audio.output.resume();
                }
            }
            Command::Pause => {
                clock.pause();
                if let Some(audio) = self.audio.as_ref() {
                    audio.output.pause();
                }
            }
            Command::Seek(position) => {
                self.seek_to(position);
                self.pending.clear();
                self.drained = false;
                if let Some(audio) = self.audio.as_mut() {
                    audio.decoder.flush();
                    audio.output.rebase(position);
                }
                clock.rebase(position);
            }
            Command::Muted(muted) => {
                if let Some(audio) = self.audio.as_ref() {
                    audio.output.set_muted(muted);
                }
            }
            Command::Stop => {}
        }
    }

    fn sync_clock(&self, clock: &mut Clock) {
        if let Some(audio) = self.audio.as_ref() {
            clock.observe(audio.output.position());
        }
    }

    fn drain_commands(&mut self, inbox: &Receiver<Command>, clock: &mut Clock) -> Flow {
        loop {
            match inbox.try_recv() {
                Ok(Command::Stop) | Err(mpsc::TryRecvError::Disconnected) => return Flow::Stop,
                Ok(command) => self.apply(command, clock),
                Err(mpsc::TryRecvError::Empty) => return Flow::Continue,
            }
        }
    }

    fn audio_is_full(&self) -> bool {
        self.audio
            .as_ref()
            .is_some_and(|audio| audio.output.is_full())
    }

    fn top_up(&mut self) {
        while self.pending.len() < MAX_PENDING_FRAMES && !self.drained && !self.audio_is_full() {
            match self.next_packet() {
                Some(packet) => self.decode_into_pending(&packet),
                None => self.drained = true,
            }
        }
    }

    fn decode_into_pending(&mut self, packet: &Packet) {
        if self.decoder.send_packet(packet).is_err() {
            return;
        }
        let mut frame = VideoFrame::empty();
        while self.decoder.receive_frame(&mut frame).is_ok() {
            let position = self.position_of(&frame);
            if let Some(rgb) = self.scale_to_rgb(&frame) {
                self.pending.push_back(Pending { rgb, position });
            }
        }
    }

    fn pump(
        &mut self,
        inbox: &Receiver<Command>,
        sink: &(dyn Fn(PlayerEvent<'_>) + Send),
        clock: &mut Clock,
    ) -> Flow {
        if matches!(self.drain_commands(inbox, clock), Flow::Stop) {
            return Flow::Stop;
        }
        self.sync_clock(clock);
        if clock.is_paused() {
            return self.wait_for_command(inbox, clock);
        }

        self.top_up();
        self.sync_clock(clock);

        let Some(next) = self.pending.front() else {
            return if self.drained {
                Flow::Ended
            } else {
                Flow::Continue
            };
        };
        let position = next.position;

        if clock.is_late(position) {
            self.pending.pop_front();
            return Flow::Continue;
        }
        if clock.due_in(position).is_zero() {
            if let Some(frame) = self.pending.pop_front() {
                sink(PlayerEvent::Frame {
                    rgb: &frame.rgb,
                    width: self.width,
                    height: self.height,
                    position,
                });
            }
            return Flow::Continue;
        }

        match inbox.recv_timeout(COMMAND_POLL) {
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => Flow::Stop,
            Ok(command) => {
                self.apply(command, clock);
                Flow::Continue
            }
            Err(RecvTimeoutError::Timeout) => Flow::Continue,
        }
    }

    fn scale_to_rgb(&mut self, frame: &VideoFrame) -> Option<Vec<u8>> {
        let mut rgb = VideoFrame::empty();
        self.scaler.run(frame, &mut rgb).ok()?;
        let row_bytes = self.width as usize * 3;
        let stride = rgb.stride(0);
        let mut packed = Vec::with_capacity(row_bytes * self.height as usize);
        for row in rgb.data(0).chunks_exact(stride).take(self.height as usize) {
            packed.extend_from_slice(row.get(..row_bytes)?);
        }
        Some(packed)
    }

    fn next_packet(&mut self) -> Option<Packet> {
        loop {
            let (stream, packet) = self.input.packets().next()?;
            let index = stream.index();
            if index == self.stream_index {
                return Some(packet);
            }
            if let Some(audio) = self.audio.as_mut()
                && index == audio.stream_index
            {
                audio.feed(&packet);
            }
        }
    }
}

enum Flow {
    Continue,
    Stop,
    Ended,
}
