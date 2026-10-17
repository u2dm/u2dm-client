use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::domain::call::identity::MediaIdentity;
use crate::domain::call::keys::{KeyIndex, MediaKey};
use crate::domain::call::media::{MediaEvent, MediaSetup};
use crate::domain::call::sfu::{SfuGrant, SfuId};
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
    async fn set_own_key(&mut self, index: KeyIndex, key: &MediaKey);
    async fn use_own_key(&mut self, index: KeyIndex);
    async fn set_key(&mut self, identities: &[MediaIdentity], index: KeyIndex, key: &MediaKey);
    async fn close(self: Box<Self>);
}
