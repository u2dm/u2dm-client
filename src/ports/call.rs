use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::domain::call::devices::{DeviceChoice, DeviceKind};
use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::keys::{KeyIndex, MediaKey};
use crate::domain::call::media::{MediaEvent, MediaSetup};
use crate::domain::call::sfu::{SfuGrant, SfuId};
use crate::domain::call::video::{FeedId, FrameSize, Framing};
use crate::error::Result;

#[async_trait]
pub trait CallMediaPort: Send + Sync {
    fn broken(&self) -> Option<String>;
    async fn open(
        &self,
        setup: MediaSetup,
        events_tx: mpsc::UnboundedSender<MediaEvent>,
    ) -> Result<Box<dyn MediaSession>>;
}

#[async_trait]
pub trait MediaSession: Send {
    async fn connect(&mut self, grant: SfuGrant) -> Result<()>;
    async fn disconnect(&mut self, sfu: &SfuId);
    async fn set_microphone(&mut self, on: bool);
    async fn set_camera(&mut self, on: bool);
    async fn set_screen_share(&mut self, on: bool);
    async fn use_device(&mut self, kind: DeviceKind, choice: DeviceChoice);
    async fn refresh_devices(&mut self);
    async fn set_own_key(&mut self, index: KeyIndex, key: &MediaKey);
    async fn use_own_key(&mut self, index: KeyIndex);
    async fn set_key(&mut self, identities: &[MediaIdentity], index: KeyIndex, key: &MediaKey);
    async fn close(self: Box<Self>);
}

pub trait VideoFrame {
    fn size(&self) -> FrameSize;
    fn write_rgba(&self, rgba: &mut [u8]) -> bool;
}

pub type FrameSink = Arc<dyn Fn(&dyn VideoFrame) + Send + Sync>;

pub trait CallVideoPort: Send + Sync {
    fn attach(&self, feed: &FeedId, wanted: FrameSize, framing: Framing, sink: FrameSink);
    fn resize(&self, feed: &FeedId, wanted: FrameSize);
    fn detach(&self, feed: &FeedId);
}
