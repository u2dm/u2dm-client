use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;

use crate::domain::models::Session;
use crate::error::{AppError, Result};
use crate::ports::storage::StoragePort;

pub struct JsonFileStorage {
    session_path: PathBuf,
}

impl JsonFileStorage {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            session_path: data_dir.join("session.json"),
        }
    }
}

#[async_trait]
impl StoragePort for JsonFileStorage {
    async fn save_session(&self, session: &Session) -> Result<()> {
        if let Some(parent) = self.session_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::Storage(e.to_string()))?;
        }
        let json = serde_json::to_string_pretty(session)?;
        fs::write(&self.session_path, json)
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn load_session(&self) -> Result<Option<Session>> {
        match fs::read_to_string(&self.session_path).await {
            Ok(contents) => {
                let session: Session = serde_json::from_str(&contents)?;
                Ok(Some(session))
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(AppError::Storage(e.to_string())),
        }
    }

    async fn clear_session(&self) -> Result<()> {
        match fs::remove_file(&self.session_path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AppError::Storage(e.to_string())),
        }
    }
}
