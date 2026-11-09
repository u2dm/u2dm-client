use std::path::PathBuf;

use directories::ProjectDirs;

use crate::error::{AppError, Result};

pub const APP_ID: &str = "chat.u2dm.Client";

const SETTINGS_FILE: &str = "settings.json";

#[allow(clippy::struct_field_names)]
pub struct AppConfig {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub config_dir: PathBuf,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        let dirs = ProjectDirs::from("", "", "U2DM")
            .ok_or_else(|| AppError::Config("Failed to determine data directory".into()))?;
        Ok(Self {
            data_dir: dirs.data_dir().to_path_buf(),
            cache_dir: dirs.cache_dir().to_path_buf(),
            config_dir: dirs.config_dir().to_path_buf(),
        })
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join(SETTINGS_FILE)
    }
}
