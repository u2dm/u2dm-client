mod camera;
mod connection;
mod devices;
mod e2ee;
mod feeds;
mod quality;
mod reception;
mod screen;
mod session;

use std::sync::{Arc, Mutex, Once};

use async_trait::async_trait;
use livekit::prelude::PlatformAudio;
use rustls::crypto::aws_lc_rs;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::task::spawn_blocking;
use tracing::Instrument;

use self::devices::Listing;
use self::feeds::Feeds;
use self::session::LiveKitSession;
use crate::domain::call::devices::DeviceLists;
use crate::domain::call::media::{MediaEvent, MediaSetup};
use crate::domain::call::video::{FeedId, FrameSize, Framing, ScreenShare};
use crate::error::{AppError, Result};
use crate::ports::call::{CallMediaPort, CallVideoPort, FrameSink, MediaSession};

static TLS_PROVIDER: Once = Once::new();

pub struct LiveKitMedia {
    broken: Arc<Mutex<Option<String>>>,
    feeds: Arc<Feeds>,
}

impl LiveKitMedia {
    pub fn new() -> Self {
        TLS_PROVIDER.call_once(|| {
            if aws_lc_rs::default_provider().install_default().is_err() {
                tracing::debug!("a TLS crypto provider was already installed");
            }
        });
        Self {
            broken: Arc::new(Mutex::new(None)),
            feeds: Arc::new(Feeds::default()),
        }
    }
}

impl CallVideoPort for LiveKitMedia {
    fn attach(&self, feed: &FeedId, wanted: FrameSize, framing: Framing, sink: FrameSink) {
        self.feeds.attach(feed, wanted, framing, sink);
    }

    fn resize(&self, feed: &FeedId, wanted: FrameSize) {
        self.feeds.resize(feed, wanted);
    }

    fn detach(&self, feed: &FeedId) {
        self.feeds.detach(feed);
    }
}

#[async_trait]
impl CallMediaPort for LiveKitMedia {
    fn broken(&self) -> Option<String> {
        self.broken.lock().ok().and_then(|broken| broken.clone())
    }

    async fn list_devices(&self) -> Result<DeviceLists> {
        let listing = spawn_blocking(|| PlatformAudio::new().map(|audio| Listing::read(&audio)))
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        listing
            .map(Listing::into_lists)
            .map_err(|e| AppError::Other(format!("the audio devices did not open: {e}")))
    }

    async fn open(
        &self,
        setup: MediaSetup,
        events_tx: mpsc::UnboundedSender<MediaEvent>,
    ) -> Result<Box<dyn MediaSession>> {
        let audio = spawn_blocking(PlatformAudio::new)
            .await
            .map_err(|e| AppError::Other(e.to_string()))?;
        let audio = match audio {
            Ok(audio) => audio,
            Err(e) => {
                let why = format!("the audio devices did not open: {e}");
                tracing::warn!("{why}");
                if let Ok(mut broken) = self.broken.lock() {
                    *broken = Some(why.clone());
                }
                return Err(AppError::Other(why));
            }
        };
        tell_screen_refusal(events_tx.clone());
        self.feeds.clear();
        self.feeds.bind_runtime(Handle::current());
        let mut session = LiveKitSession::new(
            audio,
            setup.encrypted,
            setup.devices,
            Arc::clone(&self.feeds),
            events_tx,
        );
        session.refresh_devices().await;
        Ok(Box::new(session))
    }
}

fn tell_screen_refusal(events_tx: mpsc::UnboundedSender<MediaEvent>) {
    tokio::spawn(
        async move {
            let Some(refusal) = screen::refusal().await else {
                return;
            };
            let unavailable = MediaEvent::Screen(ScreenShare::Unavailable(refusal));
            if events_tx.send(unavailable).is_err() {
                tracing::debug!("the call media closed before the screen-share check ended");
            }
        }
        .in_current_span(),
    );
}

fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}
