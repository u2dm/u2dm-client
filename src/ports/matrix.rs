use async_trait::async_trait;

use crate::domain::models::ServerInfo;
use crate::error::Result;

#[async_trait]
pub trait MatrixPort: Send + Sync {
    async fn discover_auth(&self, homeserver: &str) -> Result<ServerInfo>;
}
