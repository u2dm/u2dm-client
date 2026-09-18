use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use ffmpeg_next::{Packet, Rational, format, media};

use super::decoder;
use super::feed::AudioFeed;
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

pub fn start(path: &Path, preferred: Sound, sink: AudioSink) -> Option<Playback> {
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
    Device(AudioFeed),
    Wall { clock: Clock, heard_until: Duration },
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
        let audio = AudioFeed::open(input, output)?;
        audio.output().pause();
        Some(Self::Device(audio))
    }

    fn sound(&self) -> Sound {
        match self {
            Self::Device(_) => Sound::Device,
            Self::Wall { .. } => Sound::Silent,
        }
    }

    fn position(&self) -> Duration {
        match self {
            Self::Device(audio) => audio.output().position(),
            Self::Wall { clock, .. } => clock.elapsed(),
        }
    }

    fn resume(&mut self) {
        match self {
            Self::Device(audio) => audio.output().resume(),
            Self::Wall { clock, .. } => clock.resume(),
        }
    }

    fn pause(&mut self) {
        match self {
            Self::Device(audio) => audio.output().pause(),
            Self::Wall { clock, .. } => clock.pause(),
        }
    }

    fn set_muted(&self, muted: bool) {
        if let Self::Device(audio) = self {
            audio.output().set_muted(muted);
        }
    }

    fn rebase(&mut self, position: Duration) {
        match self {
            Self::Device(audio) => audio.rebase(position),
            Self::Wall { clock, heard_until } => {
                clock.rebase(position);
                *heard_until = position;
            }
        }
    }

    fn is_saturated(&self) -> bool {
        match self {
            Self::Device(audio) => audio.output().is_full(),
            Self::Wall { clock, heard_until } => *heard_until > clock.elapsed() + WALL_LOOKAHEAD,
        }
    }

    fn is_exhausted(&self) -> bool {
        match self {
            Self::Device(audio) => audio.output().queued_frames() == 0,
            Self::Wall { clock, heard_until } => clock.elapsed() >= *heard_until,
        }
    }

    fn feed(&mut self, packet: &Packet, time_base: Rational) {
        match self {
            Self::Device(audio) => audio.feed(packet),
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
        if let Self::Device(audio) = self {
            audio.finish();
        }
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
