use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use ffmpeg_next::Packet;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::software::scaling;
use ffmpeg_next::util::frame::Video as VideoFrame;
use ffmpeg_next::{Rational, codec, format, media};

use super::decoder;
use super::feed::AudioFeed;
use super::output::AudioOutput;
use super::playback::{Clock, Command, Flow, Playback, stream_duration};

const MAX_DIMENSION: u32 = 1280;
const COMMAND_POLL: Duration = Duration::from_millis(4);
const MAX_PENDING_FRAMES: usize = 4;
const AUDIO_INTERLEAVE_SLACK: Duration = Duration::from_secs(1);
const MAX_LOOKAHEAD_PACKETS: usize = 120;

enum AudioSupply {
    Buffered,
    Starving,
    Exhausted,
}

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

pub fn start(path: &Path, sink: Sink) -> Option<Playback> {
    let path = path.to_path_buf();
    Playback::spawn("u2dm-video", move |inbox| run(&path, inbox, sink.as_ref()))
}

fn run(path: &PathBuf, inbox: &Receiver<Command>, sink: &(dyn Fn(PlayerEvent<'_>) + Send)) {
    match Session::open(path) {
        Some(mut session) => session.drive(inbox, sink),
        None => sink(PlayerEvent::Failed),
    }
}

fn open_audio(input: &format::context::Input) -> Option<AudioFeed> {
    input.streams().best(media::Type::Audio)?;
    AudioFeed::open(input, AudioOutput::open()?)
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
    audio: Option<AudioFeed>,
    pending: VecDeque<Pending>,
    stashed: VecDeque<Packet>,
    demuxed: Duration,
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
        let audio = open_audio(&input);
        if let Some(audio) = audio.as_ref() {
            tracing::debug!(
                sample_rate = audio.output().sample_rate(),
                channels = audio.output().channels(),
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
            stashed: VecDeque::new(),
            demuxed: Duration::ZERO,
            drained: false,
        })
    }

    fn position_of(&self, frame: &VideoFrame) -> Duration {
        let ticks = frame.pts().or_else(|| frame.timestamp()).unwrap_or(0);
        decoder::ticks_to_duration(ticks, self.time_base)
    }

    fn has_finished(&self) -> bool {
        self.drained && self.pending.is_empty() && self.stashed.is_empty()
    }

    fn paced_by_audio(&self) -> bool {
        self.audio.is_some()
    }

    fn audio_supply(&self) -> AudioSupply {
        let Some(audio) = self.audio.as_ref() else {
            return AudioSupply::Exhausted;
        };
        if audio.output().queued_frames() > 0 {
            AudioSupply::Buffered
        } else if self.drained || self.looked_past_the_audio(audio) {
            AudioSupply::Exhausted
        } else {
            AudioSupply::Starving
        }
    }

    fn looked_past_the_audio(&self, audio: &AudioFeed) -> bool {
        self.demuxed >= audio.horizon().saturating_add(AUDIO_INTERLEAVE_SLACK)
            || self.stashed.len() >= MAX_LOOKAHEAD_PACKETS
    }

    fn seek_to(&mut self, position: Duration, clock: &mut Clock) {
        let target = i64::try_from(position.as_micros()).unwrap_or(i64::MAX);
        if self.input.seek(target, ..target).is_err() {
            tracing::debug!("seek failed, staying where we are");
            return;
        }
        self.decoder.flush();
        self.pending.clear();
        self.stashed.clear();
        self.drained = false;
        self.demuxed = position;
        if let Some(audio) = self.audio.as_mut() {
            audio.rebase(position);
        }
        if self.paced_by_audio() {
            clock.follow_audio(position);
        } else {
            clock.rebase(position);
        }
    }

    fn drive(&mut self, inbox: &Receiver<Command>, sink: &(dyn Fn(PlayerEvent<'_>) + Send)) {
        sink(PlayerEvent::Ready {
            duration: self.duration,
        });
        let mut clock = if self.paced_by_audio() {
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
                        audio.output().pause();
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
                if self.has_finished() {
                    self.seek_to(Duration::ZERO, clock);
                }
                clock.resume();
                if let Some(audio) = self.audio.as_ref() {
                    audio.output().resume();
                }
            }
            Command::Pause => {
                clock.pause();
                if let Some(audio) = self.audio.as_ref() {
                    audio.output().pause();
                }
            }
            Command::Seek(position) => self.seek_to(position, clock),
            Command::Muted(muted) => {
                if let Some(audio) = self.audio.as_ref() {
                    audio.output().set_muted(muted);
                }
            }
            Command::Stop => {}
        }
    }

    fn sync_clock(&self, clock: &mut Clock) {
        match self.audio_supply() {
            AudioSupply::Buffered | AudioSupply::Starving => {
                if let Some(audio) = self.audio.as_ref() {
                    clock.observe(audio.output().position());
                }
            }
            AudioSupply::Exhausted => clock.hand_off_to_wall(),
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
            .is_some_and(|audio| audio.output().is_full())
    }

    fn top_up(&mut self) {
        loop {
            self.decode_stashed();
            if !self.wants_packet() {
                return;
            }
            match self.next_packet() {
                Some(packet) => self.stashed.push_back(packet),
                None => self.close_input(),
            }
        }
    }

    fn wants_packet(&self) -> bool {
        if self.drained || self.audio_is_full() {
            return false;
        }
        self.pending.len() + self.stashed.len() < MAX_PENDING_FRAMES
            || matches!(self.audio_supply(), AudioSupply::Starving)
    }

    fn decode_stashed(&mut self) {
        while self.pending.len() < MAX_PENDING_FRAMES {
            let Some(packet) = self.stashed.pop_front() else {
                return;
            };
            self.decode_into_pending(&packet);
        }
    }

    fn close_input(&mut self) {
        if let Some(audio) = self.audio.as_mut() {
            audio.finish();
        }
        self.drained = true;
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
            let (index, time_base, packet) = {
                let (stream, packet) = self.input.packets().next()?;
                (stream.index(), stream.time_base(), packet)
            };
            self.demuxed = self.demuxed.max(packet_position(&packet, time_base));
            if index == self.stream_index {
                return Some(packet);
            }
            if let Some(audio) = self.audio.as_mut()
                && index == audio.stream_index()
            {
                audio.feed(&packet);
            }
        }
    }
}

fn packet_position(packet: &Packet, time_base: Rational) -> Duration {
    let ticks = packet.pts().or_else(|| packet.dts()).unwrap_or(0);
    decoder::ticks_to_duration(ticks, time_base)
}
