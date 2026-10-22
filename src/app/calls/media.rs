use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior, interval_at};

use crate::app::event::{AppEvent, CallEvent};
use crate::app::input::EventSender;
use crate::domain::call::devices::{DeviceChoice, DeviceKind};
use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::keys::{KeyIndex, MediaKey};
use crate::domain::call::media::{MediaEvent, MediaSetup};
use crate::domain::call::sfu::{SfuGrant, SfuId};
use crate::ports::call::{CallMediaPort, MediaSession};

const DEVICE_POLL: Duration = Duration::from_secs(2);

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

pub(super) struct MediaDriver {
    pub(super) port: Arc<dyn CallMediaPort>,
    pub(super) setup: MediaSetup,
    pub(super) events: EventSender,
    pub(super) call: u64,
}

impl MediaDriver {
    pub(super) async fn run(self, mut commands: mpsc::UnboundedReceiver<MediaCommand>) {
        let (media_tx, media_rx) = mpsc::unbounded_channel();
        let mut session = match self.port.open(self.setup.clone(), media_tx).await {
            Ok(session) => session,
            Err(e) => {
                tracing::warn!("call media did not open: {e}");
                self.send(CallEvent::MediaFailed {
                    call: self.call,
                    why: e.to_string(),
                });
                return;
            }
        };
        let forward = forward_media(&self.events, self.call, media_rx);
        let apply = async {
            let mut devices = interval_at(Instant::now() + DEVICE_POLL, DEVICE_POLL);
            devices.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    command = commands.recv() => match command {
                        Some(command) => apply(session.as_mut(), command).await,
                        None => break,
                    },
                    _ = devices.tick() => session.refresh_devices().await,
                }
            }
        };
        tokio::select! {
            () = forward => {}
            () = apply => {}
        }
        session.close().await;
        tracing::debug!(call = self.call, "closed the call media");
    }

    fn send(&self, event: CallEvent) {
        if self.events.send(AppEvent::Call(event)).is_err() {
            tracing::debug!("the app stopped before hearing from the call media");
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
        MediaCommand::Connect(grant) => {
            let sfu = grant.sfu.clone();
            if let Err(e) = session.connect(*grant).await {
                tracing::warn!(%sfu, "could not connect to the call's media server: {e}");
            }
        }
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
