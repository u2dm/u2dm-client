use std::f64::consts::PI;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use livekit::prelude::{PlatformAudio, PlayoutDeviceId, RecordingDeviceId};
use livekit::rtc_engine::lk_runtime::LkRuntime;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::AudioSourceOptions;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit::webrtc::ice_candidate::IceCandidate;
use livekit::webrtc::media_stream_track::MediaStreamTrack;
use livekit::webrtc::peer_connection::{
    AnswerOptions, OfferOptions, PeerConnection, PeerConnectionState,
};
use livekit::webrtc::peer_connection_factory::native::PeerConnectionFactoryExt;
use livekit::webrtc::peer_connection_factory::{
    ContinualGatheringPolicy, PeerConnectionFactory, RtcConfiguration,
};
use livekit::webrtc::prelude::RtcAudioTrack;
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, spawn_blocking};
use tokio::time::{Instant, sleep, timeout};

use super::devices::Listing;
use crate::domain::call::device_test::{DeviceTest, TestNews, loudness};
use crate::domain::call::devices::{DeviceChoice, DeviceKind};
use crate::ports::call::RunningTest;

const LOOPBACK_WAIT: Duration = Duration::from_secs(10);
const CANDIDATE_POLL: Duration = Duration::from_millis(20);
const SAMPLE_RATE: u32 = 48_000;
const FRAME: Duration = Duration::from_millis(10);
const FRAMES_PER_LEVEL: usize = 10;
const SOURCE_QUEUE_MS: u32 = 100;
const TEST_TRACK: &str = "device-test";
const CHIME_LOUDNESS: f64 = 0.4;
const CHIME_FADE: Duration = Duration::from_millis(20);
const CHIME: [(f64, Duration); 3] = [
    (523.25, Duration::from_millis(250)),
    (659.25, Duration::from_millis(250)),
    (783.99, Duration::from_millis(500)),
];

type Candidate = (String, i32, String);

struct Loopback {
    sender: PeerConnection,
    receiver: PeerConnection,
    tracks: mpsc::UnboundedReceiver<RtcAudioTrack>,
    for_sender: mpsc::UnboundedReceiver<Candidate>,
    for_receiver: mpsc::UnboundedReceiver<Candidate>,
}

impl Loopback {
    fn open(factory: &PeerConnectionFactory) -> Result<Self, String> {
        let mut config = RtcConfiguration::default();
        config.continual_gathering_policy = ContinualGatheringPolicy::GatherOnce;
        let sender = factory
            .create_peer_connection(config.clone())
            .map_err(|e| format!("the test connection did not open: {e}"))?;
        let receiver = factory
            .create_peer_connection(config)
            .map_err(|e| format!("the test connection did not open: {e}"))?;
        let (tracks_tx, tracks) = mpsc::unbounded_channel();
        receiver.on_track(Some(Box::new(move |event| {
            if let MediaStreamTrack::Audio(track) = event.track
                && tracks_tx.send(track).is_err()
            {
                tracing::debug!("the device test stopped waiting for its sound");
            }
        })));
        let (to_receiver, for_receiver) = mpsc::unbounded_channel();
        sender.on_ice_candidate(Some(Box::new(move |found| {
            drop(to_receiver.send(spelled(&found)));
        })));
        let (to_sender, for_sender) = mpsc::unbounded_channel();
        receiver.on_ice_candidate(Some(Box::new(move |found| {
            drop(to_sender.send(spelled(&found)));
        })));
        Ok(Self {
            sender,
            receiver,
            tracks,
            for_sender,
            for_receiver,
        })
    }

    fn send(&self, track: RtcAudioTrack) -> Result<(), String> {
        self.sender
            .add_track(MediaStreamTrack::Audio(track), &[TEST_TRACK])
            .map(drop)
            .map_err(|e| format!("the test sound could not be sent: {e}"))
    }

    async fn connect(&mut self) -> Result<RtcAudioTrack, String> {
        let offer = self
            .sender
            .create_offer(OfferOptions::default())
            .await
            .map_err(|e| format!("no test offer: {e}"))?;
        self.sender
            .set_local_description(offer.clone())
            .await
            .map_err(|e| format!("the test offer was refused: {e}"))?;
        self.receiver
            .set_remote_description(offer)
            .await
            .map_err(|e| format!("the test offer was refused: {e}"))?;
        let answer = self
            .receiver
            .create_answer(AnswerOptions::default())
            .await
            .map_err(|e| format!("no test answer: {e}"))?;
        self.receiver
            .set_local_description(answer.clone())
            .await
            .map_err(|e| format!("the test answer was refused: {e}"))?;
        self.sender
            .set_remote_description(answer)
            .await
            .map_err(|e| format!("the test answer was refused: {e}"))?;
        let deadline = Instant::now() + LOOPBACK_WAIT;
        while self.receiver.connection_state() != PeerConnectionState::Connected {
            if Instant::now() >= deadline {
                return Err(format!(
                    "the test connection did not connect within {} s",
                    LOOPBACK_WAIT.as_secs()
                ));
            }
            hand_over(&mut self.for_receiver, &self.receiver).await;
            hand_over(&mut self.for_sender, &self.sender).await;
            sleep(CANDIDATE_POLL).await;
        }
        timeout(
            deadline.saturating_duration_since(Instant::now()),
            self.tracks.recv(),
        )
        .await
        .ok()
        .flatten()
        .ok_or_else(|| "the test sound never arrived".to_owned())
    }

    fn close(&self) {
        self.sender.close();
        self.receiver.close();
    }
}

fn spelled(candidate: &IceCandidate) -> Candidate {
    (
        candidate.sdp_mid(),
        candidate.sdp_mline_index(),
        candidate.candidate(),
    )
}

async fn hand_over(candidates: &mut mpsc::UnboundedReceiver<Candidate>, to: &PeerConnection) {
    while let Ok((mid, index, sdp)) = candidates.try_recv() {
        let Ok(candidate) = IceCandidate::parse(&mid, index, &sdp) else {
            continue;
        };
        if let Err(e) = to.add_ice_candidate(candidate).await {
            tracing::debug!("a test candidate was refused: {e}");
        }
    }
}

#[derive(Clone, Copy)]
enum Restore {
    Playout,
    Recording,
}

pub(super) struct DeviceCheck {
    audio: PlatformAudio,
    factory: PeerConnectionFactory,
    loopback: Loopback,
    restore: Restore,
    work: Option<JoinHandle<()>>,
}

pub(super) async fn start(
    test: DeviceTest,
    news: mpsc::UnboundedSender<TestNews>,
) -> Result<DeviceCheck, String> {
    let audio = spawn_blocking(PlatformAudio::new)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("the audio devices did not open: {e}"))?;
    let factory = LkRuntime::instance().pc_factory().clone();
    let restore = match test {
        DeviceTest::Microphone(_) => Restore::Playout,
        DeviceTest::Speaker(_) => Restore::Recording,
    };
    switch_off(&factory, restore);
    let loopback = match Loopback::open(&factory) {
        Ok(loopback) => loopback,
        Err(why) => {
            switch_back(&factory, restore);
            return Err(why);
        }
    };
    let mut check = DeviceCheck {
        audio,
        factory,
        loopback,
        restore,
        work: None,
    };
    let started = match test {
        DeviceTest::Microphone(choice) => check.listen(&choice, news).await,
        DeviceTest::Speaker(choice) => check.play(&choice, news).await,
    };
    match started {
        Ok(()) => Ok(check),
        Err(why) => {
            Box::new(check).close().await;
            Err(why)
        }
    }
}

fn switch_off(factory: &PeerConnectionFactory, restore: Restore) {
    match restore {
        Restore::Playout => factory.set_adm_playout_enabled(false),
        Restore::Recording => factory.set_adm_recording_enabled(false),
    }
}

fn switch_back(factory: &PeerConnectionFactory, restore: Restore) {
    match restore {
        Restore::Playout => factory.set_adm_playout_enabled(true),
        Restore::Recording => factory.set_adm_recording_enabled(true),
    }
}

impl DeviceCheck {
    async fn listen(
        &mut self,
        choice: &DeviceChoice,
        news: mpsc::UnboundedSender<TestNews>,
    ) -> Result<(), String> {
        let index = self.index_of(DeviceKind::Microphone, choice).await;
        self.audio
            .set_recording_device(&RecordingDeviceId::from_index(usize::from(index)))
            .map_err(|e| format!("the microphone could not be chosen: {e}"))?;
        tracing::info!(index, "testing the microphone");
        let track = self.factory.create_device_audio_track(TEST_TRACK);
        self.loopback.send(track)?;
        let heard = self.loopback.connect().await?;
        heard.set_enabled(false);
        self.work = Some(tokio::spawn(report_levels(heard, news)));
        Ok(())
    }

    async fn play(
        &mut self,
        choice: &DeviceChoice,
        news: mpsc::UnboundedSender<TestNews>,
    ) -> Result<(), String> {
        let source = NativeAudioSource::new(
            AudioSourceOptions::default(),
            SAMPLE_RATE,
            1,
            SOURCE_QUEUE_MS,
        );
        let track = self.factory.create_audio_track(TEST_TRACK, source.clone());
        let index = self.index_of(DeviceKind::Speaker, choice).await;
        self.audio
            .set_playout_device(&PlayoutDeviceId::from_index(usize::from(index)))
            .map_err(|e| format!("the speaker could not be chosen: {e}"))?;
        tracing::info!(index, "testing the speaker");
        self.loopback.send(track)?;
        self.loopback.connect().await?;
        self.work = Some(tokio::spawn(play_chime(source, news)));
        Ok(())
    }

    async fn index_of(&self, kind: DeviceKind, choice: &DeviceChoice) -> u16 {
        let audio = self.audio.clone();
        let choice = choice.clone();
        spawn_blocking(move || {
            let listing = Listing::read(&audio);
            let device = choice.resolve(kind, listing.lists().connected.of(kind), None);
            listing.index_of(kind, device)
        })
        .await
        .unwrap_or_default()
    }
}

#[async_trait]
impl RunningTest for DeviceCheck {
    async fn close(mut self: Box<Self>) {
        if let Some(work) = self.work.take() {
            work.abort();
        }
        self.loopback.close();
        if matches!(self.restore, Restore::Playout)
            && let Err(e) = self.audio.stop_recording()
        {
            tracing::debug!("the tested microphone did not stop: {e}");
        }
        switch_back(&self.factory, self.restore);
        tracing::info!("the device test let go of its device");
    }
}

async fn report_levels(heard: RtcAudioTrack, news: mpsc::UnboundedSender<TestNews>) {
    let mut stream = NativeAudioStream::new(heard, SAMPLE_RATE.cast_signed(), 1);
    let mut window: Vec<i16> = Vec::new();
    let mut frames = 0;
    while let Some(frame) = stream.next().await {
        window.extend_from_slice(&frame.data);
        frames += 1;
        if frames < FRAMES_PER_LEVEL {
            continue;
        }
        if news.send(TestNews::Level(loudness(&window))).is_err() {
            return;
        }
        window.clear();
        frames = 0;
    }
    if news
        .send(TestNews::Failed("the microphone stopped".to_owned()))
        .is_err()
    {
        tracing::debug!("the device test closed before its microphone stopped");
    }
}

async fn play_chime(source: NativeAudioSource, news: mpsc::UnboundedSender<TestNews>) {
    for frame in chime() {
        if let Err(e) = source.capture_frame(&frame).await {
            drop(news.send(TestNews::Failed(format!("the test sound stopped: {e}"))));
            return;
        }
    }
    sleep(Duration::from_millis(u64::from(SOURCE_QUEUE_MS))).await;
    drop(news.send(TestNews::Played));
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn chime() -> Vec<AudioFrame<'static>> {
    let rate = f64::from(SAMPLE_RATE);
    let per_frame = (rate * FRAME.as_secs_f64()) as usize;
    let fade = rate * CHIME_FADE.as_secs_f64();
    let mut samples = Vec::new();
    for (pitch, length) in CHIME {
        let count = (rate * length.as_secs_f64()) as usize;
        samples.extend((0..count).map(|at| {
            let at = at as f64;
            let envelope = (at / fade).min((count as f64 - at) / fade).min(1.0);
            let wave = (2.0 * PI * pitch * at / rate).sin();
            (wave * envelope * CHIME_LOUDNESS * f64::from(i16::MAX)) as i16
        }));
    }
    samples
        .chunks(per_frame)
        .map(|chunk| {
            let mut data = chunk.to_vec();
            data.resize(per_frame, 0);
            AudioFrame {
                data: data.into(),
                sample_rate: SAMPLE_RATE,
                num_channels: 1,
                samples_per_channel: per_frame as u32,
            }
        })
        .collect()
}
