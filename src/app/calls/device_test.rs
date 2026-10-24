use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio::time::{Instant, interval_at, sleep};
use tokio_util::sync::CancellationToken;

use super::media::DEVICE_POLL;
use crate::app::event::{AppEvent, CallEvent};
use crate::app::input::EventSender;
use crate::domain::call::device_test::{DeviceTest, TestNews};
use crate::domain::call::devices::DeviceChoice;
use crate::ports::call::{CallMediaPort, RunningTest};

const TEST_LIMIT: Duration = Duration::from_secs(30);
const LEVEL_STEPS: f32 = 20.0;
const PERCENT: f32 = 100.0;

pub(super) struct Testing {
    pub(super) serial: u64,
    pub(super) test: DeviceTest,
    pub(super) state: TestState,
    stop_tx: oneshot::Sender<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TestState {
    Starting,
    Running(u8),
    Failed,
    Lost,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::app) enum TestOutcome {
    Started,
    Level(f32),
    Ended,
    Failed,
    Lost,
}

enum Step {
    Over,
    RanItsTime,
    Look,
    Heard(Option<TestNews>),
}

pub(super) struct TestTask {
    pub(super) port: Arc<dyn CallMediaPort>,
    pub(super) test: DeviceTest,
    pub(super) turn: Arc<Semaphore>,
    pub(super) events: EventSender,
    pub(super) serial: u64,
}

impl Testing {
    pub(super) fn start(
        task: TestTask,
        token: CancellationToken,
    ) -> (Self, impl Future<Output = ()> + Send + 'static) {
        tracing::info!(test = task.serial, kind = %task.test.kind(), "testing a call device");
        let (stop_tx, stopped) = oneshot::channel();
        let testing = Self {
            serial: task.serial,
            test: task.test.clone(),
            state: TestState::Starting,
            stop_tx,
        };
        (testing, task.run(stopped, token))
    }

    pub(super) fn stop(self) {
        tracing::info!(test = self.serial, "stopping the device test");
        if self.stop_tx.send(()).is_err() {
            tracing::debug!(test = self.serial, "the device test had already ended");
        }
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(super) fn level_percent(loudness: f32) -> u8 {
    ((loudness.clamp(0.0, 1.0) * LEVEL_STEPS).round() * PERCENT / LEVEL_STEPS) as u8
}

impl TestTask {
    async fn run(self, mut stopped: oneshot::Receiver<()>, token: CancellationToken) {
        let Some(turn) = self.take_turn(&mut stopped, &token).await else {
            return;
        };
        let (news_tx, news_rx) = mpsc::unbounded_channel();
        if let Some(running) = self.open(news_tx).await {
            self.follow(news_rx, &mut stopped, &token).await;
            running.close().await;
            tracing::debug!(test = self.serial, "the device test let go of its device");
        }
        drop(turn);
    }

    async fn take_turn(
        &self,
        stopped: &mut oneshot::Receiver<()>,
        token: &CancellationToken,
    ) -> Option<OwnedSemaphorePermit> {
        tokio::select! {
            biased;
            _ = stopped => None,
            () = token.cancelled() => None,
            turn = Arc::clone(&self.turn).acquire_owned() => turn
                .inspect_err(|e| tracing::debug!("the device test cannot take its turn: {e}"))
                .ok(),
        }
    }

    async fn open(&self, news: mpsc::UnboundedSender<TestNews>) -> Option<Box<dyn RunningTest>> {
        match self.port.test_device(self.test.clone(), news).await {
            Ok(running) => {
                self.tell(TestOutcome::Started);
                Some(running)
            }
            Err(e) => {
                tracing::warn!("the device test did not start: {e}");
                self.tell(TestOutcome::Failed);
                None
            }
        }
    }

    async fn follow(
        &self,
        mut news: mpsc::UnboundedReceiver<TestNews>,
        stopped: &mut oneshot::Receiver<()>,
        token: &CancellationToken,
    ) {
        let limit = sleep(TEST_LIMIT);
        tokio::pin!(limit);
        let mut presence = interval_at(Instant::now() + DEVICE_POLL, DEVICE_POLL);
        loop {
            let step = tokio::select! {
                _ = &mut *stopped => Step::Over,
                () = token.cancelled() => Step::Over,
                () = &mut limit => Step::RanItsTime,
                _ = presence.tick() => Step::Look,
                heard = news.recv() => Step::Heard(heard),
            };
            let going_on = match step {
                Step::Over => false,
                Step::RanItsTime => {
                    tracing::info!(test = self.serial, "the device test ran its time");
                    self.tell(TestOutcome::Ended);
                    false
                }
                Step::Look => self.still_there().await,
                Step::Heard(heard) => self.pass_on(heard),
            };
            if !going_on {
                return;
            }
        }
    }

    async fn still_there(&self) -> bool {
        let kind = self.test.kind();
        let (DeviceTest::Microphone(DeviceChoice::Device(tested))
        | DeviceTest::Speaker(DeviceChoice::Device(tested))) = &self.test
        else {
            return true;
        };
        match self.port.list_devices().await {
            Ok(lists) if !lists.is_connected(kind, &tested.key) => {
                tracing::info!(test = self.serial, %kind, "the tested device was disconnected");
                self.tell(TestOutcome::Lost);
                false
            }
            Ok(_) => true,
            Err(e) => {
                tracing::debug!("the devices could not be listed during a test: {e}");
                true
            }
        }
    }

    fn pass_on(&self, heard: Option<TestNews>) -> bool {
        match heard {
            Some(TestNews::Level(loudness)) => {
                self.tell(TestOutcome::Level(loudness));
                true
            }
            Some(TestNews::Played) => {
                tracing::info!(test = self.serial, "the test sound played");
                self.tell(TestOutcome::Ended);
                false
            }
            Some(TestNews::Failed(why)) => {
                tracing::warn!(test = self.serial, "the device test failed: {why}");
                self.tell(TestOutcome::Failed);
                false
            }
            None => false,
        }
    }

    fn tell(&self, outcome: TestOutcome) {
        let event = CallEvent::DeviceTest {
            test: self.serial,
            outcome,
        };
        if self.events.send(AppEvent::Call(event)).is_err() {
            tracing::debug!("the app stopped before hearing from the device test");
        }
    }
}
