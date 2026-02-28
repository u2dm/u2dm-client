use std::path::PathBuf;

use async_trait::async_trait;
use matrix_sdk::Client;
use tokio::sync::Mutex;

use crate::domain::models::{AuthMethod, ServerInfo};
use crate::error::{AppError, Result};
use crate::ports::matrix::MatrixPort;

pub struct MatrixAdapter {
    data_dir: PathBuf,
    client: Mutex<Option<Client>>,
}

impl MatrixAdapter {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            client: Mutex::new(None),
        }
    }
}

#[async_trait]
impl MatrixPort for MatrixAdapter {
    async fn discover_auth(&self, homeserver: &str) -> Result<ServerInfo> {
        let client = Client::builder()
            .server_name_or_homeserver_url(homeserver)
            .sqlite_store(self.data_dir.join("matrix-store"), None)
            .build()
            .await
            .map_err(|e| AppError::Matrix(e.to_string()))?;

        let mut methods = Vec::new();

        if client.oauth().server_metadata().await.is_ok() {
            methods.push(AuthMethod::OAuth);
        }

        if let Ok(login_types) = client.matrix_auth().get_login_types().await {
            methods.extend(
                login_types
                    .flows
                    .iter()
                    .filter_map(|f| AuthMethod::from_login_type(f.login_type())),
            );
        }

        let homeserver_url = client.homeserver().to_string();
        *self.client.lock().await = Some(client);

        Ok(ServerInfo {
            auth_methods: methods,
            homeserver_url,
        })
    }
}
