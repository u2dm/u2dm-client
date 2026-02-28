use async_trait::async_trait;

use crate::domain::models::{LoginCredentials, ServerInfo, Session};
use crate::error::Result;

#[async_trait]
pub trait MatrixPort: Send + Sync {
    async fn discover_auth(&self, homeserver: &str) -> Result<ServerInfo>;
    async fn login_password(&self, creds: LoginCredentials) -> Result<Session>;
}
