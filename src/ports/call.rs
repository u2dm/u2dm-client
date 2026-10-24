use std::result;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

use crate::domain::call::device_test::{DeviceTest, TestNews};
use crate::domain::call::devices::{DeviceChoice, DeviceKind, DeviceLists};
use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::keys::{KeyIndex, MediaKey};
use crate::domain::call::media::{MediaEvent, MediaSetup};
use crate::domain::call::sfu::{SfuGrant, SfuId, SfuRole};
use crate::domain::call::video::{FeedId, FrameSize, Framing, PreviewFailure};
use crate::error::Result;

#[async_trait]
pub trait CallMediaPort: Send + Sync {
    fn broken(&self) -> Option<String>;
    async fn list_devices(&self) -> Result<DeviceLists>;
    async fn preview_camera(
        &self,
        camera: DeviceChoice,
        lost: oneshot::Sender<String>,
    ) -> result::Result<Box<dyn CameraPreview>, PreviewFailure>;
    async fn test_device(
        &self,
        test: DeviceTest,
        news: mpsc::UnboundedSender<TestNews>,
    ) -> Result<Box<dyn RunningTest>>;
    async fn open(
        &self,
        setup: MediaSetup,
        events_tx: mpsc::UnboundedSender<MediaEvent>,
    ) -> Result<Box<dyn MediaSession>>;
}

#[async_trait]
pub trait CameraPreview: Send {
    fn feed(&self) -> FeedId;
    async fn close(self: Box<Self>);
}

#[async_trait]
pub trait RunningTest: Send {
    async fn close(self: Box<Self>);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedSfu {
    pub sfu: SfuId,
    pub role: SfuRole,
}

#[async_trait]
pub trait MediaSession: Send {
    fn dial(&mut self, grant: SfuGrant);
    async fn next_connected(&mut self) -> ConnectedSfu;
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
