mod connection;
mod e2ee;
mod session;

use std::sync::{Arc, Mutex, Once};

use async_trait::async_trait;
use livekit::prelude::PlatformAudio;
use rustls::crypto::aws_lc_rs;
use tokio::sync::mpsc;
use tokio::task::spawn_blocking;

use self::session::LiveKitSession;
use crate::domain::call::media::{MediaEvent, MediaSetup};
use crate::error::{AppError, Result};
use crate::ports::call::{CallMediaPort, MediaSession};

static TLS_PROVIDER: Once = Once::new();

pub struct LiveKitMedia {
    broken: Arc<Mutex<Option<String>>>,
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
        }
    }
}

#[async_trait]
impl CallMediaPort for LiveKitMedia {
    fn broken(&self) -> Option<String> {
        self.broken.lock().ok().and_then(|broken| broken.clone())
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
        log_devices(&audio);
        Ok(Box::new(LiveKitSession::new(
            audio,
            setup.encrypted,
            events_tx,
        )))
    }
}

fn log_devices(audio: &PlatformAudio) {
    let inputs: Vec<String> = audio
        .recording_devices()
        .map(|device| device.name)
        .collect();
    let outputs: Vec<String> = audio.playout_devices().map(|device| device.name).collect();
    tracing::info!(
        ?inputs,
        ?outputs,
        "the call uses the system's default microphone and speaker"
    );
}

fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}
