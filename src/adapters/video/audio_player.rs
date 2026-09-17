use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use ffmpeg_next::{Packet, Rational, format, media};

use super::decoder::{self, AudioDecoder, PcmTarget};
use super::output::AudioOutput;
use super::playback::{Clock, Command, Flow, Playback, stream_duration};

const POSITION_TICK: Duration = Duration::from_millis(50);
const WALL_LOOKAHEAD: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    Device,
    Silent,
}

pub enum AudioEvent {
    Ready {
        duration: Option<Duration>,
        sound: Sound,
    },
    Position(Duration),
    Ended,
    Failed,
}

pub type AudioSink = Box<dyn Fn(AudioEvent) + Send>;

type SinkRef<'a> = &'a (dyn Fn(AudioEvent) + Send);

pub fn start(path: &Path, preferred: Sound, sink: AudioSink) -> Playback {
    let path = path.to_path_buf();
    Playback::spawn("u2dm-audio", move |inbox| {
        run(&path, preferred, inbox, sink.as_ref());
    })
}

fn run(path: &PathBuf, preferred: Sound, inbox: &Receiver<Command>, sink: SinkRef<'_>) {
    match Session::open(path, preferred) {
        Some(mut session) => session.drive(inbox, sink),
        None => sink(AudioEvent::Failed),
    }
}

enum Pacing {
    Device {
        decoder: AudioDecoder,
        output: AudioOutput,
        trim_until: Option<Duration>,
    },
    Wall {
        clock: Clock,
        heard_until: Duration,
    },
}

impl Pacing {
    fn open(input: &format::context::Input, preferred: Sound) -> Option<Self> {
        let device = match preferred {
            Sound::Device => AudioOutput::open(),
            Sound::Silent => None,
        };
        let Some(output) = device else {
            return Some(Self::Wall {
                clock: Clock::wall(),
                heard_until: Duration::ZERO,
            });
        };
        let target = PcmTarget {
            rate: output.sample_rate(),
            channels: output.channels(),
        };
        let decoder = AudioDecoder::open(input, target)?;
        output.pause();
        Some(Self::Device {
            decoder,
            output,
            trim_until: None,
        })
    }

    fn sound(&self) -> Sound {
        match self {
            Self::Device { .. } => Sound::Device,
            Self::Wall { .. } => Sound::Silent,
        }
    }

    fn position(&self) -> Duration {
        match self {
            Self::Device { output, .. } => output.position(),
            Self::Wall { clock, .. } => clock.elapsed(),
        }
    }

    fn resume(&mut self) {
        match self {
            Self::Device { output, .. } => output.resume(),
            Self::Wall { clock, .. } => clock.resume(),
        }
    }

    fn pause(&mut self) {
        match self {
            Self::Device { output, .. } => output.pause(),
            Self::Wall { clock, .. } => clock.pause(),
        }
    }

    fn set_muted(&self, muted: bool) {
        if let Self::Device { output, .. } = self {
            output.set_muted(muted);
        }
    }

    fn rebase(&mut self, position: Duration) {
        match self {
            Self::Device {
                decoder,
                output,
                trim_until,
            } => {
                decoder.reset();
                output.rebase(position);
                *trim_until = Some(position);
            }
            Self::Wall { clock, heard_until } => {
                clock.rebase(position);
                *heard_until = position;
            }
        }
    }

    fn is_saturated(&self) -> bool {
        match self {
            Self::Device { output, .. } => output.is_full(),
            Self::Wall { clock, heard_until } => *heard_until > clock.elapsed() + WALL_LOOKAHEAD,
        }
    }

    fn is_exhausted(&self) -> bool {
        match self {
            Self::Device { output, .. } => output.queued_frames() == 0,
            Self::Wall { clock, heard_until } => clock.elapsed() >= *heard_until,
        }
    }

    fn feed(&mut self, packet: &Packet, time_base: Rational) {
        match self {
            Self::Device {
                decoder,
                output,
                trim_until,
            } => decoder.feed(packet, &mut |samples, start| {
                push_trimmed(output, trim_until, samples, start);
            }),
            Self::Wall { heard_until, .. } => {
                let ticks = packet
                    .pts()
                    .unwrap_or(0)
                    .saturating_add(packet.duration().max(0));
                *heard_until = (*heard_until).max(decoder::ticks_to_duration(ticks, time_base));
            }
        }
    }

    fn finish(&mut self) {
        if let Self::Device {
            decoder,
            output,
            trim_until,
        } = self
        {
            decoder.finish(&mut |samples, start| {
                push_trimmed(output, trim_until, samples, start);
            });
        }
    }
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

struct Session {
    input: format::context::Input,
    stream_index: usize,
    time_base: Rational,
    duration: Option<Duration>,
    pacing: Pacing,
    drained: bool,
    paused: bool,
    ended: bool,
}

impl Session {
    fn open(path: &PathBuf, preferred: Sound) -> Option<Self> {
        if !super::ffmpeg_ready() {
            return None;
        }
        let input = format::input(path).ok()?;
        let stream = input.streams().best(media::Type::Audio)?;
        let stream_index = stream.index();
        let time_base = stream.time_base();
        let duration = stream_duration(&input);
        let pacing = Pacing::open(&input, preferred)?;
        tracing::debug!(
            silent = matches!(pacing.sound(), Sound::Silent),
            "audio playback opened"
        );
        Some(Self {
            input,
            stream_index,
            time_base,
            duration,
            pacing,
            drained: false,
            paused: true,
            ended: false,
        })
    }

    fn drive(&mut self, inbox: &Receiver<Command>, sink: SinkRef<'_>) {
        sink(AudioEvent::Ready {
            duration: self.duration,
            sound: self.pacing.sound(),
        });
        let mut reported = Instant::now();
        loop {
            if matches!(self.drain_commands(inbox, sink), Flow::Stop) {
                return;
            }
            if self.paused || self.ended {
                if matches!(self.wait_for_command(inbox, sink), Flow::Stop) {
                    return;
                }
                continue;
            }
            self.top_up();
            if reported.elapsed() >= POSITION_TICK {
                sink(AudioEvent::Position(self.position()));
                reported = Instant::now();
            }
            if self.drained && self.pacing.is_exhausted() {
                self.end(sink);
                continue;
            }
            if matches!(self.poll(inbox, sink), Flow::Stop) {
                return;
            }
        }
    }

    fn poll(&mut self, inbox: &Receiver<Command>, sink: SinkRef<'_>) -> Flow {
        match inbox.recv_timeout(POSITION_TICK) {
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => Flow::Stop,
            Ok(command) => {
                self.apply(command, sink);
                Flow::Continue
            }
            Err(RecvTimeoutError::Timeout) => Flow::Continue,
        }
    }

    fn drain_commands(&mut self, inbox: &Receiver<Command>, sink: SinkRef<'_>) -> Flow {
        loop {
            match inbox.try_recv() {
                Ok(Command::Stop) | Err(mpsc::TryRecvError::Disconnected) => return Flow::Stop,
                Ok(command) => self.apply(command, sink),
                Err(mpsc::TryRecvError::Empty) => return Flow::Continue,
            }
        }
    }

    fn wait_for_command(&mut self, inbox: &Receiver<Command>, sink: SinkRef<'_>) -> Flow {
        match inbox.recv() {
            Ok(Command::Stop) | Err(_) => Flow::Stop,
            Ok(command) => {
                self.apply(command, sink);
                Flow::Continue
            }
        }
    }

    fn apply(&mut self, command: Command, sink: SinkRef<'_>) {
        match command {
            Command::Play => {
                if self.ended {
                    self.seek_to(Duration::ZERO);
                }
                self.paused = false;
                self.pacing.resume();
            }
            Command::Pause => {
                self.paused = true;
                self.pacing.pause();
            }
            Command::Seek(position) => self.seek_to(position),
            Command::Muted(muted) => self.pacing.set_muted(muted),
            Command::Stop => return,
        }
        sink(AudioEvent::Position(self.position()));
    }

    fn seek_to(&mut self, position: Duration) {
        let position = self.duration.map_or(position, |end| position.min(end));
        let target = i64::try_from(position.as_micros()).unwrap_or(i64::MAX);
        if self.input.seek(target, ..target).is_err() {
            tracing::debug!("audio seek failed, staying where we are");
            return;
        }
        self.drained = false;
        self.ended = false;
        self.pacing.rebase(position);
    }

    fn position(&self) -> Duration {
        let heard = self.pacing.position();
        self.duration.map_or(heard, |end| heard.min(end))
    }

    fn top_up(&mut self) {
        while !self.drained && !self.pacing.is_saturated() {
            if let Some(packet) = self.next_packet() {
                self.pacing.feed(&packet, self.time_base);
            } else {
                self.pacing.finish();
                self.drained = true;
            }
        }
    }

    fn next_packet(&mut self) -> Option<Packet> {
        loop {
            let (stream, packet) = self.input.packets().next()?;
            if stream.index() == self.stream_index {
                return Some(packet);
            }
        }
    }

    fn end(&mut self, sink: SinkRef<'_>) {
        self.pacing.pause();
        self.ended = true;
        sink(AudioEvent::Position(self.position()));
        sink(AudioEvent::Ended);
    }
}
