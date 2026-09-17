use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ffmpeg_next::format;

const LATE_FRAME_TOLERANCE: Duration = Duration::from_millis(120);

#[derive(Clone, Copy)]
pub(super) enum Command {
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
    pub(super) fn spawn<F>(name: &str, body: F) -> Self
    where
        F: FnOnce(&Receiver<Command>) + Send + 'static,
    {
        let (commands, inbox) = mpsc::channel();
        let worker = thread::Builder::new()
            .name(name.into())
            .spawn(move || body(&inbox))
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
            tracing::debug!("the playback thread is gone, dropping the command");
        }
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.send(Command::Stop);
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::warn!("the playback thread panicked");
        }
    }
}

pub(super) enum Clock {
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
    pub(super) fn wall() -> Self {
        Self::Wall {
            origin: Instant::now(),
            paused_at: Some(Instant::now()),
        }
    }

    pub(super) fn audio() -> Self {
        Self::Audio {
            position: Duration::ZERO,
            paused: true,
        }
    }

    pub(super) fn is_paused(&self) -> bool {
        match self {
            Self::Wall { paused_at, .. } => paused_at.is_some(),
            Self::Audio { paused, .. } => *paused,
        }
    }

    pub(super) fn resume(&mut self) {
        match self {
            Self::Wall { origin, paused_at } => {
                if let Some(at) = paused_at.take() {
                    *origin += at.elapsed();
                }
            }
            Self::Audio { paused, .. } => *paused = false,
        }
    }

    pub(super) fn pause(&mut self) {
        match self {
            Self::Wall { paused_at, .. } => {
                if paused_at.is_none() {
                    *paused_at = Some(Instant::now());
                }
            }
            Self::Audio { paused, .. } => *paused = true,
        }
    }

    pub(super) fn rebase(&mut self, to: Duration) {
        match self {
            Self::Wall { origin, paused_at } => {
                let now = paused_at.unwrap_or_else(Instant::now);
                *origin = now.checked_sub(to).unwrap_or(now);
            }
            Self::Audio { position, .. } => *position = to,
        }
    }

    pub(super) fn observe(&mut self, heard: Duration) {
        if let Self::Audio { position, .. } = self {
            *position = heard;
        }
    }

    pub(super) fn elapsed(&self) -> Duration {
        match self {
            Self::Wall { origin, paused_at } => paused_at
                .unwrap_or_else(Instant::now)
                .saturating_duration_since(*origin),
            Self::Audio { position, .. } => *position,
        }
    }

    pub(super) fn due_in(&self, position: Duration) -> Duration {
        position.saturating_sub(self.elapsed())
    }

    pub(super) fn is_late(&self, position: Duration) -> bool {
        self.elapsed() > position + LATE_FRAME_TOLERANCE
    }
}

pub(super) enum Flow {
    Continue,
    Stop,
    Ended,
}

pub(super) fn stream_duration(input: &format::context::Input) -> Option<Duration> {
    let micros = input.duration();
    (micros > 0).then(|| Duration::from_micros(micros.unsigned_abs()))
}
