use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ffmpeg_next::format::Pixel;
use ffmpeg_next::software::scaling;
use ffmpeg_next::util::frame::Video as VideoFrame;
use ffmpeg_next::{Rational, codec, format, media};

const MAX_DIMENSION: u32 = 1280;
const LATE_FRAME_TOLERANCE: Duration = Duration::from_millis(120);
const COMMAND_POLL: Duration = Duration::from_millis(4);

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

struct Clock {
    origin: Instant,
    paused_at: Option<Instant>,
}

impl Clock {
    fn started_at(position: Duration) -> Self {
        Self {
            origin: Instant::now().checked_sub(position).unwrap_or_else(Instant::now),
            paused_at: Some(Instant::now()),
        }
    }

    fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    fn resume(&mut self) {
        if let Some(paused_at) = self.paused_at.take() {
            self.origin += paused_at.elapsed();
        }
    }

    fn pause(&mut self) {
        if self.paused_at.is_none() {
            self.paused_at = Some(Instant::now());
        }
    }

    fn rebase(&mut self, position: Duration) {
        let now = self.paused_at.unwrap_or_else(Instant::now);
        self.origin = now.checked_sub(position).unwrap_or(now);
    }

    fn due_in(&self, position: Duration) -> Option<Duration> {
        let deadline = self.origin.checked_add(position)?;
        Some(deadline.saturating_duration_since(Instant::now()))
    }

    fn is_late(&self, position: Duration) -> bool {
        self.origin
            .checked_add(position)
            .is_some_and(|deadline| Instant::now() > deadline + LATE_FRAME_TOLERANCE)
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

struct Session {
    input: format::context::Input,
    decoder: codec::decoder::Video,
    scaler: scaling::Context,
    stream_index: usize,
    time_base: Rational,
    duration: Option<Duration>,
    width: u32,
    height: u32,
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
        Some(Self {
            input,
            decoder,
            scaler,
            stream_index,
            time_base,
            duration,
            width,
            height,
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
        let mut clock = Clock::started_at(Duration::ZERO);
        loop {
            match self.pump(inbox, sink, &mut clock) {
                Flow::Continue => {}
                Flow::Stop => return,
                Flow::Ended => {
                    sink(PlayerEvent::Ended);
                    clock.pause();
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
            Command::Play => clock.resume(),
            Command::Pause => clock.pause(),
            Command::Seek(position) => {
                self.seek_to(position);
                clock.rebase(position);
            }
            Command::Stop => {}
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

    fn pump(
        &mut self,
        inbox: &Receiver<Command>,
        sink: &(dyn Fn(PlayerEvent<'_>) + Send),
        clock: &mut Clock,
    ) -> Flow {
        if matches!(self.drain_commands(inbox, clock), Flow::Stop) {
            return Flow::Stop;
        }
        if clock.is_paused() {
            return self.wait_for_command(inbox, clock);
        }
        let Some(packet) = self.next_packet() else {
            return Flow::Ended;
        };
        if self.decoder.send_packet(&packet).is_err() {
            return Flow::Continue;
        }
        let mut frame = VideoFrame::empty();
        while self.decoder.receive_frame(&mut frame).is_ok() {
            let position = self.position_of(&frame);
            if clock.is_late(position) {
                continue;
            }
            match self.hold_until(inbox, clock, position) {
                Flow::Stop => return Flow::Stop,
                Flow::Ended => return Flow::Ended,
                Flow::Continue => {}
            }
            self.emit(&frame, position, sink);
        }
        Flow::Continue
    }

    fn hold_until(
        &mut self,
        inbox: &Receiver<Command>,
        clock: &mut Clock,
        position: Duration,
    ) -> Flow {
        while let Some(remaining) = clock.due_in(position) {
            if remaining.is_zero() {
                return Flow::Continue;
            }
            match inbox.recv_timeout(remaining.min(COMMAND_POLL)) {
                Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => return Flow::Stop,
                Ok(command) => {
                    self.apply(command, clock);
                    if clock.is_paused() && matches!(self.wait_for_command(inbox, clock), Flow::Stop)
                    {
                        return Flow::Stop;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
        Flow::Continue
    }

    fn next_packet(&mut self) -> Option<ffmpeg_next::Packet> {
        loop {
            let (stream, packet) = self.input.packets().next()?;
            if stream.index() == self.stream_index {
                return Some(packet);
            }
        }
    }

    fn emit(
        &mut self,
        frame: &VideoFrame,
        position: Duration,
        sink: &(dyn Fn(PlayerEvent<'_>) + Send),
    ) {
        let mut rgb = VideoFrame::empty();
        if self.scaler.run(frame, &mut rgb).is_err() {
            return;
        }
        let row_bytes = self.width as usize * 3;
        let stride = rgb.stride(0);
        let mut packed = Vec::with_capacity(row_bytes * self.height as usize);
        for row in rgb.data(0).chunks_exact(stride).take(self.height as usize) {
            let Some(row) = row.get(..row_bytes) else {
                return;
            };
            packed.extend_from_slice(row);
        }
        sink(PlayerEvent::Frame {
            rgb: &packed,
            width: self.width,
            height: self.height,
            position,
        });
    }
}

enum Flow {
    Continue,
    Stop,
    Ended,
}
