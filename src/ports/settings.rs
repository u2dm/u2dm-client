use async_trait::async_trait;

use crate::domain::settings::Settings;
use crate::error::Result;

#[async_trait]
pub trait SettingsPort: Send + Sync {
    fn load(&self) -> Settings;
    async fn save(&self, settings: &Settings) -> Result<()>;
}
