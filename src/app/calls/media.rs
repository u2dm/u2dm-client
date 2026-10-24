use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::time::{Instant, MissedTickBehavior, interval_at, timeout};

use crate::app::event::{AppEvent, CallEvent};
use crate::app::input::EventSender;
use crate::domain::call::devices::{DeviceChoice, DeviceKind};
use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::keys::{KeyIndex, MediaKey};
use crate::domain::call::media::{MediaEvent, MediaSetup};
use crate::domain::call::sfu::{SfuGrant, SfuId, SfuRole};
use crate::ports::call::{CallMediaPort, ConnectedSfu, MediaSession};

pub(super) const DEVICE_POLL: Duration = Duration::from_secs(2);
pub(super) const MEDIA_TURN_SHARES: u32 = 2;
const MEDIA_HANDOVER_WAIT: Duration = Duration::from_secs(10);

pub(super) enum MediaCommand {
    Connect(Box<SfuGrant>),
    Disconnect(SfuId),
    Microphone(bool),
    Camera(bool),
    ScreenShare(bool),
    Device(DeviceKind, DeviceChoice),
    RefreshDevices,
    OwnKey(KeyIndex, MediaKey),
    UseOwnKey(KeyIndex),
    Key(Vec<MediaIdentity>, KeyIndex, MediaKey),
}

#[derive(Debug, Clone, Copy, Default)]
struct MediaWants {
    microphone: bool,
    camera: bool,
}

impl MediaWants {
    const fn follow(&mut self, command: &MediaCommand) {
        match command {
            MediaCommand::Microphone(on) => self.microphone = *on,
            MediaCommand::Camera(on) => self.camera = *on,
            MediaCommand::Connect(_)
            | MediaCommand::Disconnect(_)
            | MediaCommand::ScreenShare(_)
            | MediaCommand::Device(..)
            | MediaCommand::RefreshDevices
            | MediaCommand::OwnKey(..)
            | MediaCommand::UseOwnKey(_)
            | MediaCommand::Key(..) => {}
        }
    }

    async fn send_on(self, session: &mut dyn MediaSession, connected: &ConnectedSfu) {
        if connected.role != SfuRole::Publish {
            return;
        }
        tracing::debug!(
            sfu = %connected.sfu,
            microphone = self.microphone,
            camera = self.camera,
            "sending what the call wants now that it is connected"
        );
        if self.microphone {
            session.set_microphone(true).await;
        }
        if self.camera {
            session.set_camera(true).await;
        }
    }
}

pub(super) struct MediaDriver {
    pub(super) port: Arc<dyn CallMediaPort>,
    pub(super) setup: MediaSetup,
    pub(super) events: EventSender,
    pub(super) call: u64,
    pub(super) turn: Arc<Semaphore>,
}

impl MediaDriver {
    pub(super) async fn run(self, mut commands: mpsc::UnboundedReceiver<MediaCommand>) {
        let Some(turn) = self.take_turn(&commands).await else {
            return;
        };
        let (media_tx, media_rx) = mpsc::unbounded_channel();
        let Some(mut session) = self.open(&commands, media_tx).await else {
            return;
        };
        let forward = forward_media(&self.events, self.call, media_rx);
        let drive = drive(session.as_mut(), &mut commands);
        tokio::select! {
            () = forward => {}
            () = drive => {}
        }
        session.close().await;
        drop(turn);
        tracing::debug!(call = self.call, "closed the call media");
    }

    async fn open(
        &self,
        commands: &mpsc::UnboundedReceiver<MediaCommand>,
        media_tx: mpsc::UnboundedSender<MediaEvent>,
    ) -> Option<Box<dyn MediaSession>> {
        if commands.is_closed() {
            tracing::debug!(call = self.call, "the call ended before its media opened");
            return None;
        }
        match self.port.open(self.setup.clone(), media_tx).await {
            Ok(session) => Some(session),
            Err(e) => {
                tracing::warn!("call media did not open: {e}");
                self.send(CallEvent::MediaFailed {
                    call: self.call,
                    why: e.to_string(),
                });
                None
            }
        }
    }

    async fn take_turn(
        &self,
        commands: &mpsc::UnboundedReceiver<MediaCommand>,
    ) -> Option<OwnedSemaphorePermit> {
        let every_share = Arc::clone(&self.turn).acquire_many_owned(MEDIA_TURN_SHARES);
        match timeout(MEDIA_HANDOVER_WAIT, every_share).await {
            Ok(Ok(turn)) => Some(turn),
            Ok(Err(e)) => {
                tracing::debug!(call = self.call, "the call media cannot take its turn: {e}");
                None
            }
            Err(_) if commands.is_closed() => {
                tracing::debug!(
                    call = self.call,
                    "the call ended while its media waited for the last one to close"
                );
                None
            }
            Err(_) => {
                tracing::warn!(
                    call = self.call,
                    "the previous call's media did not close in time"
                );
                self.send(CallEvent::MediaFailed {
                    call: self.call,
                    why: "the previous call's media did not close".into(),
                });
                None
            }
        }
    }

    fn send(&self, event: CallEvent) {
        if self.events.send(AppEvent::Call(event)).is_err() {
            tracing::debug!("the app stopped before hearing from the call media");
        }
    }
}

async fn drive(
    session: &mut dyn MediaSession,
    commands: &mut mpsc::UnboundedReceiver<MediaCommand>,
) {
    let mut wants = MediaWants::default();
    let mut devices = interval_at(Instant::now() + DEVICE_POLL, DEVICE_POLL);
    devices.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            command = commands.recv() => match command {
                Some(command) if !commands.is_closed() => {
                    wants.follow(&command);
                    apply(session, command).await;
                }
                Some(_) | None => return,
            },
            connected = session.next_connected() => wants.send_on(session, &connected).await,
            _ = devices.tick() => session.refresh_devices().await,
        }
    }
}

async fn forward_media(
    events: &EventSender,
    call: u64,
    mut media_rx: mpsc::UnboundedReceiver<MediaEvent>,
) {
    while let Some(event) = media_rx.recv().await {
        if events
            .send(AppEvent::Call(CallEvent::Media { call, event }))
            .is_err()
        {
            return;
        }
    }
}

async fn apply(session: &mut dyn MediaSession, command: MediaCommand) {
    match command {
        MediaCommand::Connect(grant) => session.dial(*grant),
        MediaCommand::Disconnect(sfu) => session.disconnect(&sfu).await,
        MediaCommand::Microphone(on) => session.set_microphone(on).await,
        MediaCommand::Camera(on) => session.set_camera(on).await,
        MediaCommand::ScreenShare(on) => session.set_screen_share(on).await,
        MediaCommand::Device(kind, choice) => session.use_device(kind, choice).await,
        MediaCommand::RefreshDevices => session.refresh_devices().await,
        MediaCommand::OwnKey(index, key) => session.set_own_key(index, &key).await,
        MediaCommand::UseOwnKey(index) => session.use_own_key(index).await,
        MediaCommand::Key(identities, index, key) => {
            session.set_key(&identities, index, &key).await;
        }
    }
}
